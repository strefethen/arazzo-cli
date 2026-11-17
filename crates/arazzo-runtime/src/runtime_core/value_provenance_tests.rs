#![allow(clippy::unwrap_used)]

use super::payload::resolve_value_source_with_provenance;
use super::value_provenance::{ProvenancedValues, ValueProducer, ValueProvenance};
use super::*;

fn marked(value: Value) -> ProvenancedValues {
    ProvenancedValues {
        values: BTreeMap::from([("x".into(), value)]),
        provenance: BTreeMap::from([(
            "x".into(),
            ValueProvenance {
                unresolved: true,
                ..Default::default()
            },
        )]),
    }
}

#[test]
fn atomic_overwrite_restore_and_snapshot() {
    let mut vars = VarStore::default();
    assert!(vars
        .replace_step_outputs("s", marked(json!({"ok": 1})))
        .is_none());
    let snapshot = vars.clone();
    let previous = vars.replace_step_outputs("s", ProvenancedValues::default());
    assert!(vars.step_values("s").values.is_empty());
    vars.restore_step_outputs("s", previous);
    assert_eq!(vars.step_values("s"), snapshot.step_values("s"));
    vars.set_step_output("s", "x", json!(7));
    assert!(vars.step_values("s").provenance.is_empty());
    assert!(snapshot.step_values("s").provenance["x"].unresolved);
    vars.restore_step_outputs("s", None);
    assert!(!super::eval_context::from_vars(&vars, None)
        .steps
        .contains_key("s"));
    vars.set_inputs(marked(Value::Null), BTreeSet::new());
    vars.set_input("x", json!(false));
    assert!(vars.input_values().provenance.is_empty());
}

#[test]
fn canonical_aliases_and_conservative_pointer_join() {
    let mut vars = VarStore::default();
    vars.set_inputs(marked(json!({"ok": 2})), BTreeSet::new());
    vars.replace_step_outputs("s", marked(json!({"ok": 2})));
    vars.register_workflow_state("w", marked(json!({"ok": 2})), marked(json!({"ok": 2})));
    let eval = ExpressionEvaluator::new(super::eval_context::from_vars(&vars, None));
    for source in [
        "$inputs.x#/ok",
        "$steps.s.outputs.x#/ok",
        "$workflows.w.inputs.x#/ok",
        "$workflows.w.outputs.x#/ok",
    ] {
        let resolution =
            resolve_value_source_with_provenance(&ValueSource::from(source), &eval).unwrap();
        assert_eq!(resolution.value, json!(2));
        assert!(
            vars.value_provenance(&resolution, &ProvenancedValues::default(), None)
                .unresolved,
            "{source}"
        );
    }
}

#[test]
fn dry_producer_mixed_failures_and_outgoing_response() {
    let vars = VarStore::default();
    let eval = ExpressionEvaluator::new(EvalContext {
        status_code: Some(200),
        response_body: arazzo_expr::BodySlot::Present(json!({})),
        ..Default::default()
    });
    let producer = ValueProducer {
        workflow_id: "w",
        step_id: "s",
        site: ValueProducerSite::Output("o"),
        transport: ValueProducerTransport::Http,
    };
    for source in ["$statusCode", "$response.body", "$response.body#/absent"] {
        let result =
            resolve_value_source_with_provenance(&ValueSource::from(source), &eval).unwrap();
        let metadata =
            vars.value_provenance(&result, &ProvenancedValues::default(), Some(producer));
        assert!(!metadata.unresolved, "{source}");
        assert_eq!(metadata.skipped.len(), 1);
    }
    let result = resolve_value_source_with_provenance(
        &ValueSource::from("{$response.body#/absent}-{$inputs.absent}"),
        &eval,
    )
    .unwrap();
    let metadata = vars.value_provenance(&result, &ProvenancedValues::default(), Some(producer));
    assert!(metadata.unresolved);
    assert_eq!(metadata.skipped.len(), 1);
    let outgoing = ExpressionEvaluator::new(EvalContext::default());
    let result =
        resolve_value_source_with_provenance(&ValueSource::from("$response.body"), &outgoing)
            .unwrap();
    let metadata = vars.value_provenance(&result, &ProvenancedValues::default(), None);
    assert!(metadata.unresolved);
    assert!(metadata.skipped.is_empty());
}

#[test]
fn request_projection_omissions_and_ordinary_context() {
    let mut vars = VarStore::default();
    vars.set_inputs(marked(Value::Null), BTreeSet::from(["optional".into()]));
    vars.set_input("real", Value::Null);
    vars.replace_step_outputs("s", marked(Value::Null));
    vars.register_workflow_values(
        "w",
        marked(Value::Null),
        marked(Value::Null),
        BTreeSet::from(["optional".into()]),
    );
    let ordinary = super::eval_context::from_vars(&vars, None);
    let sibling = vars.clone();
    let projected = super::request_admission::request_context(&vars, ordinary.clone(), "DELETE");
    assert!(!Arc::ptr_eq(&ordinary.inputs, &projected.inputs));
    assert!(!Arc::ptr_eq(&ordinary.steps, &projected.steps));
    assert!(!Arc::ptr_eq(&ordinary.workflows, &projected.workflows));
    assert!(Arc::ptr_eq(
        &ordinary.source_descriptions,
        &projected.source_descriptions
    ));
    assert_eq!(sibling.input_values(), vars.input_values());
    assert_eq!(sibling.step_values("s"), vars.step_values("s"));
    assert_eq!(
        sibling.workflow_provenance["w"].inputs,
        vars.workflow_provenance["w"].inputs
    );
    assert_eq!(
        sibling.workflow_provenance["w"].outputs,
        vars.workflow_provenance["w"].outputs
    );
    assert_eq!(
        sibling.workflow_provenance["w"].optional_omissions,
        vars.workflow_provenance["w"].optional_omissions
    );
    assert!(
        super::eval_context::from_vars(&sibling, None).workflows["w"]
            .inputs
            .contains_key("x")
    );
    assert_eq!(projected.method.as_deref(), Some("DELETE"));
    assert_eq!(projected.inputs.get("optional"), Some(&Value::Null));
    assert_eq!(projected.inputs.get("real"), Some(&Value::Null));
    assert!(!projected.inputs.contains_key("x"));
    assert!(!projected.inputs.contains_key("undeclared"));
    assert!(!projected.steps["s"].contains_key("x"));
    assert!(!projected.workflows["w"].inputs.contains_key("x"));
    assert!(!projected.workflows["w"].outputs.contains_key("x"));
    assert_eq!(
        projected.workflows["w"].inputs.get("optional"),
        Some(&Value::Null)
    );
    assert!(!ordinary.workflows["w"].inputs.contains_key("optional"));
    assert!(ordinary.inputs.contains_key("x"));
    assert!(!ordinary.inputs.contains_key("optional"));
    assert!(ordinary.steps["s"].contains_key("x"));
    assert!(vars.debug_scopes().inputs.contains_key("x"));
}

#[test]
fn no_op_admission_preserves_every_shared_map() {
    let mut vars = VarStore::default();
    for populated in [false, true] {
        if populated {
            vars.set_inputs(marked(Value::Null), BTreeSet::from(["optional".into()]));
            vars.set_input("x", Value::Null);
            vars.set_input("optional", Value::Null);
            vars.input_provenance.insert(
                "missing".into(),
                marked(Value::Null).provenance["x"].clone(),
            );
            vars.replace_step_outputs("s", BTreeMap::from([("real".into(), Value::Null)]));
            Arc::make_mut(&mut vars.step_provenance)
                .insert("s".into(), marked(Value::Null).provenance);
            vars.register_workflow_values(
                "w",
                marked(Value::Null),
                marked(Value::Null),
                BTreeSet::new(),
            );
            vars.register_workflow_values(
                "w",
                BTreeMap::new().into(),
                BTreeMap::new().into(),
                BTreeSet::new(),
            );
            let metadata = vars
                .workflow_provenance
                .get_mut("w")
                .unwrap_or_else(|| panic!("workflow"));
            metadata.inputs = marked(Value::Null).provenance.clone();
            metadata.outputs = marked(Value::Null).provenance;
        }
        let ordinary = super::eval_context::from_vars(&vars, None);
        let projected = super::request_admission::request_context(&vars, ordinary.clone(), "GET");
        assert!(Arc::ptr_eq(&ordinary.inputs, &projected.inputs));
        assert!(Arc::ptr_eq(&ordinary.steps, &projected.steps));
        assert!(Arc::ptr_eq(&ordinary.workflows, &projected.workflows));
        assert!(Arc::ptr_eq(
            &ordinary.source_descriptions,
            &projected.source_descriptions
        ));
    }
}

#[test]
fn admission_copies_only_the_changed_map() {
    for location in ["input", "step", "workflow"] {
        let mut vars = VarStore::default();
        match location {
            "input" => vars.set_inputs(marked(Value::Null), BTreeSet::new()),
            "step" => {
                vars.replace_step_outputs("s", marked(Value::Null));
            }
            _ => vars.register_workflow_values(
                "w",
                marked(Value::Null),
                BTreeMap::new().into(),
                BTreeSet::new(),
            ),
        }
        let ordinary = super::eval_context::from_vars(&vars, None);
        let projected = super::request_admission::request_context(&vars, ordinary.clone(), "GET");
        assert_eq!(
            Arc::ptr_eq(&ordinary.inputs, &projected.inputs),
            location != "input"
        );
        assert_eq!(
            Arc::ptr_eq(&ordinary.steps, &projected.steps),
            location != "step"
        );
        assert_eq!(
            Arc::ptr_eq(&ordinary.workflows, &projected.workflows),
            location != "workflow"
        );
        assert!(Arc::ptr_eq(
            &ordinary.source_descriptions,
            &projected.source_descriptions
        ));
    }
}

#[test]
fn schema_derived_request_omissions_preserve_required_and_supplied_unknown_absence() {
    let schema = parse(json!({
        "type": "object",
        "required": ["required"],
        "properties": {
            "required": {"type": "string"},
            "optional": {"type": "string"},
            "x": {"type": "object"}
        }
    }));
    let supplied = marked(json!({"concreteSibling": 7}));
    let omissions =
        super::value_provenance::optional_input_omissions(Some(&schema), &supplied.values);
    assert_eq!(omissions, BTreeSet::from(["optional".to_owned()]));
    let mut vars = VarStore::default();
    vars.set_inputs(supplied.clone(), omissions.clone());
    vars.register_workflow_values(
        "child",
        supplied.clone(),
        ProvenancedValues::default(),
        omissions,
    );
    let ordinary = super::eval_context::from_vars(&vars, None);
    let debug = vars.debug_scopes();
    let projected = super::request_admission::request_context(&vars, ordinary.clone(), "GET");
    for inputs in [&projected.inputs, &projected.workflows["child"].inputs] {
        assert_eq!(inputs.get("optional"), Some(&Value::Null));
        assert!(!inputs.contains_key("required"));
        assert!(!inputs.contains_key("undeclared"));
        assert!(!inputs.contains_key("x"));
    }
    for inputs in [&ordinary.inputs, &ordinary.workflows["child"].inputs] {
        assert_eq!(inputs, &supplied.values);
        assert!(!inputs.contains_key("optional"));
        assert!(!inputs.contains_key("required"));
        assert!(!inputs.contains_key("undeclared"));
    }
    let unchanged = super::eval_context::from_vars(&vars, None);
    assert_eq!(unchanged.inputs, ordinary.inputs);
    assert_eq!(
        unchanged.workflows["child"].inputs,
        ordinary.workflows["child"].inputs
    );
    assert_eq!(vars.debug_scopes().inputs, debug.inputs);
    assert_eq!(projected.method.as_deref(), Some("GET"));
    assert_eq!(ordinary.method, None);
}

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap_or_else(|error| panic!("fixture: {error}"))
}

fn engine(mut workflows: Value, dry: bool, parallel: bool, strict: bool, base: &str) -> Engine {
    if let Some(workflows) = workflows.as_array_mut() {
        for workflow in workflows {
            if workflow["steps"].as_array().is_some_and(Vec::is_empty) {
                workflow["steps"] = json!([{"stepId":"noop", "operationId":"get"}]);
            }
        }
    }
    let spec = parse(json!({
        "arazzo":"1.1.0", "info":{"title":"provenance", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":base, "type":"openapi"}],
        "workflows":workflows
    }));
    EngineBuilder::new(spec)
        .dry_run(dry)
        .parallel(parallel)
        .strict_inputs(strict)
        .openapi_spec(
            json!({"openapi":"3.2.0", "$self":base, "info":{"title":"fixture", "version":"1"},
            "servers":[{"url":base}], "paths":{"/ok":{"get":{"operationId":"get"}}}})
            .to_string()
            .into_bytes(),
            None,
        )
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"))
}

fn context() -> (ExecutionContext, mpsc::Receiver<EngineEvent>) {
    let (event_tx, event_rx) = mpsc::channel(1024);
    (
        ExecutionContext {
            event_tx,
            role: ContextRole::Invocation,
            trace_seq: AtomicU64::new(0),
            run_seq: AtomicU64::new(0),
            execution_event_seq: AtomicU64::new(0),
            step_attempts: Mutex::default(),
            cancel: CancellationToken::new(),
            is_timeout: Arc::new(AtomicBool::new(false)),
            completed_workflows: Mutex::default(),
            message_receivers: Arc::default(),
        },
        event_rx,
    )
}

#[tokio::test]
async fn recursive_compounds_and_multiple_workflow_aliases_keep_skipped_origins() {
    let engine = engine(
        json!([
            {"workflowId":"main", "steps":[
                {"stepId":"producer", "operationId":"get", "outputs":{"id":"$response.body#/id"}},
                {"stepId":"child", "workflowId":"child", "parameters":[{"name":"v", "value":{"nested":"$steps.producer.outputs.id", "ok":1}},
                    {"name":"mixed", "value":["$steps.producer.outputs.id", "$inputs.absent"]}]}
            ], "outputs":{"a":"$steps.child.outputs.a#/ok", "b":"$outputs.a", "c":"$outputs.b", "registered":"$workflows.child.outputs.a#/ok", "mixed":"$steps.child.outputs.mixed"}},
            {"workflowId":"child", "steps":[], "outputs":{"a":"$inputs.v", "b":"$outputs.a", "mixed":"$inputs.mixed"}}
        ]),
        true,
        false,
        false,
        "http://api.invalid",
    );
    let (ctx, _rx) = context();
    let result = engine
        .execute_inner(&ctx, "main", ProvenancedValues::default(), 0, None)
        .await
        .unwrap_or_else(|error| panic!("recursive run: {error}"));
    for name in ["a", "b", "c", "registered"] {
        assert_eq!(result.values[name], json!(1));
        let metadata = &result.provenance[name];
        assert!(!metadata.unresolved);
        assert_eq!(metadata.skipped.len(), 1);
        assert_eq!(metadata.skipped[0].workflow_id, "main");
        assert_eq!(metadata.skipped[0].step_id, "producer");
        assert_eq!(
            metadata.skipped[0].site,
            super::value_provenance::SkippedValueSite::Output("id".into())
        );
    }
    assert!(result.provenance["mixed"].unresolved);
    assert_eq!(result.provenance["mixed"].skipped.len(), 1);
}

#[tokio::test]
async fn parallel_merge_and_real_missing_output_aliases() {
    let engine = engine(
        json!([
            {"workflowId":"main", "steps":[
                {"stepId":"first", "operationId":"get", "outputs":{"value":"$inputs.missing", "status":"$statusCode"}},
                {"stepId":"second", "operationId":"get", "outputs":{"value":"$inputs.x"}}
            ], "outputs":{"a":"$steps.first.outputs.value", "b":"$outputs.a", "c":"$steps.second.outputs.value", "status":"$steps.first.outputs.status"}}
        ]),
        true,
        true,
        false,
        "http://api.invalid",
    );
    let (ctx, _rx) = context();
    let result = engine
        .execute_inner(&ctx, "main", marked(json!(42)), 0, None)
        .await
        .unwrap_or_else(|error| panic!("parallel run: {error}"));
    assert_eq!(result.values["c"], json!(42));
    for name in ["a", "b", "c"] {
        assert!(result.provenance[name].unresolved, "{name}");
    }
    assert_eq!(result.values["status"], json!(200));
    assert!(!result.provenance["status"].unresolved);
    assert_eq!(result.provenance["status"].skipped.len(), 1);
}

#[tokio::test]
async fn goto_with_and_without_parameters_preserves_caller_carriers() {
    for parameters in [json!([]), json!([{"name":"x", "value":"$inputs.x"}])] {
        for supplied in [
            marked(json!(12)),
            BTreeMap::from([("x".into(), json!(12))]).into(),
        ] {
            let engine = engine(
                json!([
                    {"workflowId":"main", "steps":[{"stepId":"go", "operationId":"get", "onSuccess":[
                        {"name":"goto", "type":"goto", "workflowId":"target", "parameters":parameters}
                    ]}]},
                    {"workflowId":"target", "steps":[], "outputs":{"a":"$inputs.x", "b":"$outputs.a"}}
                ]),
                true,
                false,
                false,
                "http://api.invalid",
            );
            let (ctx, _rx) = context();
            let result = engine
                .execute_inner(&ctx, "main", supplied.clone(), 0, None)
                .await
                .unwrap_or_else(|error| panic!("goto run: {error}"));
            assert_eq!(result.values["b"], json!(12));
            assert_eq!(
                result.provenance.contains_key("b"),
                supplied.provenance.contains_key("x")
            );
        }
    }
}

#[tokio::test]
async fn retry_workflow_and_live_http_outputs_keep_metadata() {
    for parameters in [json!([]), json!([{"name":"x", "value":"$inputs.x"}])] {
        for supplied in [
            marked(json!(12)),
            BTreeMap::from([("x".into(), json!(12))]).into(),
        ] {
            let server = tiny_http::Server::http("127.0.0.1:0")
                .unwrap_or_else(|error| panic!("server: {error}"));
            let base = format!("http://{}", server.server_addr());
            let worker = std::thread::spawn(move || {
                for status in [500, 200, 200] {
                    let request = server
                        .recv_timeout(Duration::from_secs(10))
                        .unwrap_or_else(|error| panic!("request: {error}"))
                        .unwrap_or_else(|| panic!("request timed out"));
                    request
                        .respond(
                            tiny_http::Response::from_string("{\"nil\":null}")
                                .with_status_code(status),
                        )
                        .unwrap_or_else(|error| panic!("response: {error}"));
                }
            });
            let engine = engine(
                json!([
                    {"workflowId":"main", "steps":[{"stepId":"retry", "operationId":"get", "successCriteria":[{"condition":"$statusCode == 200"}],
                        "outputs":{"realNull":"$response.body#/nil", "missing":"$response.body#/absent"},
                        "onFailure":[{"name":"recover", "type":"retry", "workflowId":"recovery", "retryLimit":1, "retryAfter":0, "parameters":parameters}]
                    }], "outputs":{"a":"$workflows.recovery.outputs.a", "missing":"$steps.retry.outputs.missing", "realNull":"$steps.retry.outputs.realNull"}},
                    {"workflowId":"recovery", "steps":[], "outputs":{"a":"$inputs.x"}}
                ]),
                false,
                false,
                false,
                &base,
            );
            let (ctx, _rx) = context();
            let result = engine
                .execute_inner(&ctx, "main", supplied.clone(), 0, None)
                .await
                .unwrap_or_else(|error| panic!("retry run: {error}"));
            worker.join().unwrap_or_else(|_| panic!("server failed"));
            assert_eq!(result.values["a"], json!(12));
            assert_eq!(
                result.provenance.contains_key("a"),
                supplied.provenance.contains_key("x")
            );
            assert!(result.provenance["missing"].unresolved);
            assert!(result.provenance["missing"].skipped.is_empty());
            assert_eq!(result.values["realNull"], Value::Null);
            assert!(!result.provenance.contains_key("realNull"));
        }
    }
}

#[tokio::test]
async fn production_strict_child_defers_unknown_validation_to_request_consumption() {
    for consumed in [true, false] {
        let parameters = if consumed {
            json!([{"name":"x","in":"query","value":"$inputs.x"}])
        } else {
            json!([])
        };
        let engine = engine(
            json!([
                {"workflowId":"main","steps":[{"stepId":"child","workflowId":"child","parameters":[{"name":"x","value":"$inputs.missing"}]}]},
                {"workflowId":"child","inputs":{"type":"object","required":["x"],"properties":{"x":{"type":"string","enum":["ok"]}}},"steps":[{"stepId":"http","operationId":"get","parameters":parameters}]}
            ]),
            true,
            false,
            true,
            "http://api.invalid",
        );
        let (ctx, mut rx) = context();
        let result = engine
            .execute_inner(&ctx, "main", ProvenancedValues::default(), 0, None)
            .await;
        if consumed {
            assert_eq!(
                result
                    .err()
                    .unwrap_or_else(|| panic!("consumed unknown"))
                    .code(),
                "RUNTIME_UNRESOLVED_REQUEST_VALUE"
            );
        } else {
            assert!(result.is_ok(), "{result:?}");
        }
        let mut requests = 0;
        while let Ok(event) = rx.try_recv() {
            if let EngineEvent::DryRunRequest(_) = event {
                requests += 1;
            }
        }
        assert_eq!(requests, usize::from(!consumed));
    }
}

#[test]
fn skipped_producers_classify_only_their_transport_and_preserve_live_misses() {
    let vars = VarStore::default();
    let eval = ExpressionEvaluator::new(EvalContext {
        response_body: arazzo_expr::BodySlot::Present(json!({})),
        message_payload: Some(json!({})),
        ..EvalContext::default()
    });
    for transport in [
        ValueProducerTransport::Http,
        ValueProducerTransport::Message,
    ] {
        let producer = ValueProducer {
            workflow_id: "w",
            step_id: "s",
            site: ValueProducerSite::Output("o"),
            transport,
        };
        for source in [
            "$response.body#/missing",
            "$statusCode",
            "$message.payload#/missing",
            "$message.header.missing",
            "$inputs.missing",
        ] {
            let result =
                resolve_value_source_with_provenance(&ValueSource::from(source), &eval).unwrap();
            let metadata =
                vars.value_provenance(&result, &ProvenancedValues::default(), Some(producer));
            let skipped = match transport {
                ValueProducerTransport::Http => {
                    source.starts_with("$response") || source == "$statusCode"
                }
                ValueProducerTransport::Message => source.starts_with("$message"),
            };
            assert_eq!(metadata.unresolved, !skipped, "{source}");
            assert_eq!(metadata.skipped.len(), usize::from(skipped), "{source}");
            let live = vars.value_provenance(&result, &ProvenancedValues::default(), None);
            assert!(live.unresolved, "{source}");
            assert!(live.skipped.is_empty(), "{source}");
        }
        let mixed = resolve_value_source_with_provenance(
            &ValueSource::from("{$message.payload#/missing}-{$inputs.missing}"),
            &eval,
        )
        .unwrap();
        assert!(
            vars.value_provenance(&mixed, &ProvenancedValues::default(), Some(producer))
                .unresolved
        );
    }
}

#[test]
fn unknown_validation_preserves_presence_defaults_order_and_concrete_behavior() {
    let schema = parse(
        json!({"type":"object", "required":["x", "other"], "properties":{
            "x":{"type":"string", "enum":["yes"], "default":"yes"}, "other":{"type":"integer"}, "optional":{"default":3}
        }}),
    );
    for value in [json!(null), json!(17), json!("bad")] {
        let mut concrete = BTreeMap::from([
            ("x".into(), value.clone()),
            ("other".into(), json!("wrong")),
            ("extra".into(), json!(true)),
        ]);
        let mut wrapped: ProvenancedValues = concrete.clone().into();
        let public = validate_inputs(&schema, &mut concrete);
        let private = super::input_validation::validate_provenanced_inputs(&schema, &mut wrapped);
        assert_eq!(
            public
                .iter()
                .map(|issue| (issue.severity, issue.to_string()))
                .collect::<Vec<_>>(),
            private
                .iter()
                .map(|issue| (issue.severity, issue.to_string()))
                .collect::<Vec<_>>()
        );
        assert_eq!(concrete, wrapped.values);
        wrapped.provenance = marked(value.clone()).provenance;
        let issues = super::input_validation::validate_provenanced_inputs(&schema, &mut wrapped);
        assert_eq!(wrapped.values["x"], value);
        assert!(issues.iter().all(|issue| issue.field != "x"));
        assert!(issues.iter().any(|issue| issue.field == "other"));
        assert!(issues.iter().any(|issue| issue.field == "extra"));
        assert_eq!(wrapped.values["optional"], json!(3));
    }
    let schema = parse(json!({"type":"object", "required":["x"]}));
    let mut supplied = marked(Value::Null);
    assert!(
        super::input_validation::validate_provenanced_inputs(&schema, &mut supplied).is_empty()
    );
    let mut absent = ProvenancedValues::default();
    assert_eq!(
        super::input_validation::validate_provenanced_inputs(&schema, &mut absent).len(),
        1
    );
}

#[tokio::test]
async fn received_message_outputs_use_common_carrier() {
    let engine = engine(json!([]), false, false, false, "http://api.invalid");
    let (ctx, _rx) = context();
    let step = parse(json!({"stepId":"receive", "outputs":{
        "id":"$message.payload#/id", "missing":"$message.payload#/absent", "inherited":"$inputs.x", "null":"$message.payload#/nil"
    }}));
    let mut vars = VarStore::default();
    vars.set_inputs(marked(json!(8)), BTreeSet::new());
    let snapshot = step_context::MessageSnapshot {
        headers: BTreeMap::new(),
        payload: Some(json!({"id":9, "nil":null})),
    };
    let execution = engine
        .evaluate_message(
            &ctx,
            "main",
            &step,
            &vars,
            0,
            snapshot,
            TraceMessage::default(),
            true,
            Vec::new(),
        )
        .await
        .unwrap_or_else(|error| panic!("message output: {error}"));
    assert_eq!(execution.outputs.values["id"], json!(9));
    assert!(!execution.outputs.provenance.contains_key("id"));
    assert!(execution.outputs.provenance["missing"].unresolved);
    assert!(execution.outputs.provenance["inherited"].unresolved);
    assert!(!execution.outputs.provenance.contains_key("null"));
}

fn skipped(value: Value) -> ProvenancedValues {
    let mut values = marked(value);
    values
        .provenance
        .get_mut("x")
        .unwrap_or_else(|| panic!("metadata"))
        .unresolved = false;
    values
        .provenance
        .get_mut("x")
        .unwrap_or_else(|| panic!("metadata"))
        .skipped
        .push(super::value_provenance::SkippedOrigin {
            workflow_id: "producer".into(),
            step_id: "http".into(),
            site: super::value_provenance::SkippedValueSite::Output("x".into()),
            source: "$response.body#/id".into(),
        });
    values
}

#[test]
fn selector_controls_and_mixed_skipped_failures_keep_both_classes() {
    let mut vars = VarStore::default();
    vars.set_inputs(skipped(json!({"ok":1})), BTreeSet::new());
    let eval = ExpressionEvaluator::new(super::eval_context::from_vars(&vars, None));
    for (source, unresolved, skipped_count) in [
        (
            json!({"context":"$inputs.x", "selector":"/missing", "type":"jsonpointer"}),
            false,
            1,
        ),
        (
            json!({"context":"$inputs.x#/absent", "selector":"/id", "type":"jsonpointer"}),
            false,
            1,
        ),
        (
            json!({"context":"$inputs.x", "selector":"bad-pointer", "type":"jsonpointer"}),
            true,
            1,
        ),
        (
            json!({"context":"$inputs.missing", "selector":"/id", "type":"jsonpointer"}),
            true,
            0,
        ),
        (
            json!({"context":"$inputs.x#bad", "selector":"/id", "type":"jsonpointer"}),
            true,
            0,
        ),
        (json!(["$inputs.x", "$inputs.missing"]), true, 1),
        (
            json!(["$inputs.x", {"context":"$inputs.x", "selector":"[", "type":"jsonpath"}]),
            true,
            1,
        ),
    ] {
        let source: ValueSource = parse(source);
        let result = resolve_value_source_with_provenance(&source, &eval).unwrap();
        let metadata = vars.value_provenance(&result, &ProvenancedValues::default(), None);
        assert_eq!(metadata.unresolved, unresolved, "{source:?}");
        assert_eq!(metadata.skipped.len(), skipped_count, "{source:?}");
    }
    vars.set_inputs(
        BTreeMap::from([("x".into(), json!({"nil":null}))]).into(),
        BTreeSet::new(),
    );
    let eval = ExpressionEvaluator::new(super::eval_context::from_vars(&vars, None));
    for source in [
        json!(null),
        json!(""),
        json!("$inputs.x#/nil"),
        json!({"context":"$inputs.x", "selector":"/absent", "type":"jsonpointer"}),
    ] {
        let source: ValueSource = parse(source);
        let result = resolve_value_source_with_provenance(&source, &eval).unwrap();
        assert_eq!(
            vars.value_provenance(&result, &ProvenancedValues::default(), None),
            ValueProvenance::default()
        );
    }
}

#[test]
fn skipped_validation_and_request_projection_retain_values() {
    let schema = parse(
        json!({"type":"object", "required":["x"], "properties":{"x":{"type":"integer", "enum":[42], "default":42}}}),
    );
    let mut values = skipped(Value::Null);
    assert!(super::input_validation::validate_provenanced_inputs(&schema, &mut values).is_empty());
    assert_eq!(values.values["x"], Value::Null);
    let mut vars = VarStore::default();
    vars.set_inputs(values.clone(), BTreeSet::new());
    vars.register_workflow_state("child", values.clone(), values.clone());
    vars.replace_step_outputs("s", values);
    let projected = super::request_admission::request_context(
        &vars,
        super::eval_context::from_vars(&vars, None),
        "POST",
    );
    assert!(projected.inputs.contains_key("x"));
    assert!(projected.steps["s"].contains_key("x"));
    assert!(projected.workflows["child"].inputs.contains_key("x"));
    assert!(projected.workflows["child"].outputs.contains_key("x"));
}

#[tokio::test]
async fn subworkflow_publication_rolls_back_both_fields_and_records_omissions() {
    let engine = engine(
        json!([
            {"workflowId":"child", "inputs":{"type":"object", "properties":{"optional":{"type":"string"}, "x":{"type":"object"}}, "required":["x"]},
                "steps":[], "outputs":{"x":"$inputs.x"}}
        ]),
        true,
        false,
        false,
        "http://api.invalid",
    );
    let (ctx, _rx) = context();
    for existing in [None, Some(marked(json!("previous")))] {
        let mut vars = VarStore::default();
        vars.set_inputs(skipped(json!({"ok":1})), BTreeSet::new());
        if let Some(previous) = existing.clone() {
            vars.replace_step_outputs("child", previous);
        }
        let step = parse(
            json!({"stepId":"child", "workflowId":"child", "parameters":[{"name":"x", "value":"$inputs.x"}],
            "successCriteria":[{"condition":"false"}]}),
        );
        let execution = engine
            .execute_step_with_result(&ctx, "main", &step, &mut vars, 0)
            .await
            .unwrap_or_else(|error| panic!("child: {error}"));
        assert!(!execution.result.success);
        assert_eq!(
            super::eval_context::from_vars(&vars, None)
                .steps
                .contains_key("child"),
            existing.is_some()
        );
        if let Some(previous) = existing {
            assert_eq!(vars.step_values("child"), previous);
        }
        let projected = super::request_admission::request_context(
            &vars,
            super::eval_context::from_vars(&vars, None),
            "GET",
        );
        assert_eq!(
            projected.workflows["child"].inputs.get("optional"),
            Some(&Value::Null)
        );
        assert!(
            !super::eval_context::from_vars(&vars, None).workflows["child"]
                .inputs
                .contains_key("optional")
        );
        let result = resolve_value_source_with_provenance(
            &ValueSource::from("$workflows.child.outputs.x#/ok"),
            &ExpressionEvaluator::new(projected),
        )
        .unwrap();
        assert_eq!(result.value, json!(1));
        assert_eq!(
            vars.value_provenance(&result, &ProvenancedValues::default(), None)
                .skipped
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn dry_action_response_parameters_keep_explicit_producer_and_real_misses() {
    let engine = engine(
        json!([
            {"workflowId":"main", "steps":[{"stepId":"go", "operationId":"get", "onSuccess":[{"name":"go", "type":"goto", "workflowId":"target", "parameters":[
                {"name":"status", "value":"$statusCode"}, {"name":"body", "value":"$response.body#/id"}, {"name":"mixed", "value":"{$response.body#/id}-{$inputs.missing}"}
            ]}]}]},
            {"workflowId":"target", "steps":[], "outputs":{"status":"$inputs.status", "body":"$inputs.body", "mixed":"$inputs.mixed"}}
        ]),
        true,
        false,
        false,
        "http://api.invalid",
    );
    let (ctx, _rx) = context();
    let result = engine
        .execute_inner(&ctx, "main", ProvenancedValues::default(), 0, None)
        .await
        .unwrap_or_else(|error| panic!("action: {error}"));
    assert_eq!(result.values["status"], json!(200));
    assert_eq!(result.values["body"], Value::Null);
    for name in ["status", "body"] {
        assert!(!result.provenance[name].unresolved);
        assert_eq!(result.provenance[name].skipped.len(), 1);
        assert_eq!(
            result.provenance[name].skipped[0].site,
            super::value_provenance::SkippedValueSite::ActionParameter(name.into())
        );
    }
    assert!(result.provenance["mixed"].unresolved);
    assert_eq!(result.provenance["mixed"].skipped.len(), 1);
}

#[tokio::test]
async fn concrete_repeated_parameter_clears_metadata_and_json_cannot_create_it() {
    let engine = engine(
        json!([
            {"workflowId":"main", "steps":[{"stepId":"child", "workflowId":"child", "parameters":[
                {"name":"x", "value":"$inputs.absent"}, {"name":"x", "value":{"unresolved":true,"skipped":["marker"],"provenance":{"x":true}}}
            ]}], "outputs":{"x":"$steps.child.outputs.x"}},
            {"workflowId":"child", "steps":[], "outputs":{"x":"$inputs.x"}}
        ]),
        true,
        false,
        false,
        "http://api.invalid",
    );
    let (ctx, _rx) = context();
    let result = engine
        .execute_inner(&ctx, "main", ProvenancedValues::default(), 0, None)
        .await
        .unwrap_or_else(|error| panic!("repeat: {error}"));
    assert_eq!(
        result.values["x"],
        json!({"unresolved":true,"skipped":["marker"],"provenance":{"x":true}})
    );
    assert!(result.provenance.is_empty());
    let mut vars = VarStore::default();
    vars.register_workflow_state("child", marked(Value::Null), marked(Value::Null));
    let snapshot = vars.clone();
    vars.register_workflow_state(
        "child",
        BTreeMap::from([("x".into(), json!("concrete"))]),
        BTreeMap::new(),
    );
    let eval = ExpressionEvaluator::new(super::eval_context::from_vars(&vars, None));
    let result = resolve_value_source_with_provenance(
        &ValueSource::from("$workflows.child.inputs.x"),
        &eval,
    )
    .unwrap();
    assert_eq!(
        vars.value_provenance(&result, &ProvenancedValues::default(), None),
        ValueProvenance::default()
    );
    assert!(snapshot.workflow_provenance["child"].inputs["x"].unresolved);
}

#[test]
fn legacy_output_projections_retain_typed_xpath_and_dotted_failures() {
    let vars = VarStore::default();
    let response = Response {
        status_code: 200,
        headers: BTreeMap::new(),
        body: b"<root><id>1</id></root>".to_vec(),
        body_json: None,
        content_type: ContentType::Xml,
        redirects: Vec::new(),
    };
    let eval = ExpressionEvaluator::new(super::eval_context::from_vars(&vars, Some(&response)));
    for (expression, unresolved) in [("//id", false), ("//missing", false), ("//*[", true)] {
        let output = OutputValue::RuntimeExpression(expression.into());
        let (value, warnings, metadata) =
            evaluate_provenanced_output(&output, &eval, Some(&response), &vars, None).unwrap();
        let ordinary = evaluate_output_value_detailed(&output, &eval, Some(&response)).unwrap();
        assert_eq!((value, warnings), ordinary);
        assert_eq!(metadata.unresolved, unresolved, "{expression}");
    }
    let eval = ExpressionEvaluator::new(EvalContext {
        response_body: arazzo_expr::BodySlot::Present(json!({"id":3})),
        ..Default::default()
    });
    let output = OutputValue::RuntimeExpression("id".into());
    let (_, _, metadata) = evaluate_provenanced_output(
        &output,
        &eval,
        None,
        &vars,
        Some(ValueProducer {
            workflow_id: "w",
            step_id: "s",
            site: ValueProducerSite::Output("out"),
            transport: ValueProducerTransport::Http,
        }),
    )
    .unwrap();
    assert!(metadata.unresolved);
    assert!(metadata.skipped.is_empty());
}
