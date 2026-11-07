use super::*;

pub(super) fn from_vars(vars: &VarStore, response: Option<&Response>) -> EvalContext {
    let mut ctx = EvalContext {
        inputs: vars.inputs_snapshot(),
        steps: vars.steps_snapshot(),
        workflows: vars.workflows_snapshot(),
        ..EvalContext::default()
    };
    if let Some(resp) = response {
        ctx.status_code = Some(resp.status_code);
        ctx.response_headers = resp.headers.clone();
        ctx.response_body = resp
            .body_json
            .clone()
            .or_else(|| String::from_utf8(resp.body.clone()).ok().map(Value::String));
    }
    ctx
}

pub(super) fn base(engine: &Engine, vars: &VarStore, response: Option<&Response>) -> EvalContext {
    let mut ctx = from_vars(vars, response);
    ctx.self_uri = engine.inner.index.spec.self_uri.clone();
    ctx.source_descriptions = Arc::clone(&engine.inner.index.source_descriptions_map);
    ctx
}

pub(super) fn request(engine: &Engine, vars: &VarStore, method: &str) -> EvalContext {
    super::request_admission::request_context(vars, base(engine, vars, None), method)
}

pub(super) fn post_request(
    engine: &Engine,
    vars: &VarStore,
    response: Option<&Response>,
    prep: &super::engine_http::PreparedRequest,
) -> EvalContext {
    let mut ctx = base(engine, vars, response);
    ctx.method = Some(prep.method.clone());
    ctx.url = Some(prep.url_result.url.clone());
    ctx.request_headers = prep.headers.clone();
    ctx.request_query = prep.url_result.query_params.clone();
    ctx.request_path = prep.url_result.path_params.clone();
    ctx.request_body = prep.body_json.clone();
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        let spec = serde_yaml_ng::from_str("arazzo: 1.1.0\ninfo: {title: snapshots, version: 1.0.0}\nsourceDescriptions: []\nworkflows: []\n")
            .unwrap_or_else(|err| panic!("fixture: {err}"));
        EngineBuilder::new(spec)
            .build()
            .unwrap_or_else(|err| panic!("engine: {err}"))
    }

    fn assert_shared(left: &EvalContext, right: &EvalContext) {
        assert!(Arc::ptr_eq(&left.inputs, &right.inputs));
        assert!(Arc::ptr_eq(&left.steps, &right.steps));
        assert!(Arc::ptr_eq(&left.workflows, &right.workflows));
        assert!(Arc::ptr_eq(
            &left.source_descriptions,
            &right.source_descriptions
        ));
    }

    #[test]
    fn response_overlay_preserves_absence_json_text_and_invalid_utf8() {
        let engine = engine();
        let vars = VarStore::default();
        let absent = base(&engine, &vars, None);
        assert_eq!(absent.status_code, None);
        assert_eq!(absent.response_body, None);
        assert!(absent.response_headers.is_empty());
        for (body, body_json, expected) in [
            (b"raw".to_vec(), Some(Value::Null), Some(Value::Null)),
            (b"<xml/>".to_vec(), None, Some(json!("<xml/>"))),
            (vec![255], None, None),
        ] {
            let response = Response {
                status_code: 201,
                headers: BTreeMap::from([("X-Test".into(), "value".into())]),
                body,
                body_json,
                content_type: ContentType::Xml,
                redirects: Vec::new(),
            };
            let context = base(&engine, &vars, Some(&response));
            assert_eq!(context.status_code, Some(201));
            assert_eq!(context.response_headers, response.headers);
            assert_eq!(context.response_body, expected);
            assert_shared(&absent, &context);
        }
    }

    #[test]
    fn standalone_url_helper_retains_empty_method_binding() {
        let engine = engine();
        let step = Step {
            step_id: "helper".into(),
            parameters: vec![Parameter {
                name: "method".into(),
                in_: Some(ParamLocation::Query),
                value: "$method".into(),
                ..Parameter::default()
            }],
            ..Step::default()
        };
        let url = engine
            .build_url_from_path(
                "/value",
                "https://example.test",
                &step,
                &VarStore::default(),
            )
            .unwrap_or_else(|err| panic!("url: {err}"));
        assert_eq!(url.query_params["method"], "");
        assert_eq!(url.url, "https://example.test/value?method=");
    }

    #[test]
    fn unchanged_owners_and_context_clones_share_allocations() {
        let engine = engine();
        let mut vars = VarStore::default();
        vars.set_input("i", json!({"nested": [1, 2]}));
        vars.set_step_output("s", "v", json!(3));
        vars.register_workflow_state(
            "w",
            BTreeMap::from([("i".into(), json!(4))]),
            BTreeMap::new(),
        );
        let first = base(&engine, &vars, None);
        let second = base(&engine, &vars, None);
        assert_shared(&first, &second);
        assert_shared(&first, &first.clone());
        assert!(Arc::ptr_eq(&first.inputs, &vars.inputs_snapshot()));
        assert!(Arc::ptr_eq(&first.steps, &vars.steps_snapshot()));
        assert!(Arc::ptr_eq(&first.workflows, &vars.workflows_snapshot()));
        assert!(Arc::ptr_eq(
            &first.source_descriptions,
            &engine.inner.index.source_descriptions_map
        ));
    }

    #[test]
    fn owner_mutations_isolate_retained_and_sibling_snapshots() {
        let engine = engine();
        let mut vars = VarStore::default();
        vars.set_input("i", json!(1));
        vars.set_step_output("s", "v", json!(2));
        vars.register_workflow_state(
            "w",
            BTreeMap::new(),
            BTreeMap::from([("o".into(), json!(3))]),
        );
        let sibling = vars.clone();
        let old = base(&engine, &vars, None);
        vars.set_inputs(
            BTreeMap::from([("i".into(), json!(10))]).into(),
            BTreeSet::new(),
        );
        let inputs = base(&engine, &vars, None);
        assert_eq!(old.inputs["i"], json!(1));
        assert_eq!(inputs.inputs["i"], json!(10));
        assert!(!Arc::ptr_eq(&old.inputs, &inputs.inputs));
        assert!(Arc::ptr_eq(&old.workflows, &inputs.workflows));
        assert!(Arc::ptr_eq(&old.steps, &inputs.steps));
        vars.register_workflow_state(
            "w",
            BTreeMap::new(),
            BTreeMap::from([("o".into(), json!(30))]),
        );
        let workflow = base(&engine, &vars, None);
        assert_eq!(old.workflows["w"].outputs["o"], json!(3));
        assert_eq!(workflow.workflows["w"].outputs["o"], json!(30));
        assert!(!Arc::ptr_eq(&inputs.workflows, &workflow.workflows));
        assert!(Arc::ptr_eq(&inputs.inputs, &workflow.inputs));
        assert!(Arc::ptr_eq(&inputs.steps, &workflow.steps));
        vars.set_step_output("s", "v", json!(20));
        let step = base(&engine, &vars, None);
        assert_eq!(old.steps["s"]["v"], json!(2));
        assert_eq!(step.steps["s"]["v"], json!(20));
        assert!(!Arc::ptr_eq(&workflow.steps, &step.steps));
        assert!(Arc::ptr_eq(&workflow.inputs, &step.inputs));
        assert!(Arc::ptr_eq(&workflow.workflows, &step.workflows));
        assert!(Arc::ptr_eq(
            &old.source_descriptions,
            &step.source_descriptions
        ));
        assert_shared(&old, &base(&engine, &sibling, None));
        vars.register_workflow_state("new", BTreeMap::new(), BTreeMap::new());
        assert!(!old.workflows.contains_key("new"));
        assert!(base(&engine, &vars, None).workflows.contains_key("new"));
    }
}
