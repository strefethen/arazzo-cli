use super::*;

pub(super) fn from_vars(vars: &VarStore, response: Option<&Response>) -> EvalContext {
    let mut ctx = EvalContext {
        inputs: vars.inputs_snapshot(),
        blob_slots: vars.blob_slots(),
        steps: vars.steps_snapshot(),
        workflows: vars.workflows_snapshot(),
        ..EvalContext::default()
    };
    if let Some(resp) = response {
        ctx.status_code = Some(resp.status_code);
        ctx.response_headers = resp.headers.clone();
        ctx.response_body = resp.body_value.clone();
        ctx.blob_slots.response_body = resp.body_blob.is_some();
    }
    #[cfg(test)]
    if let Some(reason) = &vars.unreadable_response_body {
        ctx.response_body = arazzo_expr::BodySlot::Unavailable {
            reason: reason.clone(),
        };
    }
    ctx
}

pub(super) fn base(engine: &Engine, vars: &VarStore, response: Option<&Response>) -> EvalContext {
    let mut ctx = from_vars(vars, response);
    ctx.self_uri = engine.inner.index.spec.self_uri.clone();
    ctx.source_descriptions = Arc::clone(&engine.inner.index.source_descriptions_map);
    ctx
}

pub(super) fn after_attempt(
    engine: &Engine,
    vars: &VarStore,
    response: Option<&Response>,
    request: Option<&TraceRequest>,
) -> EvalContext {
    let mut ctx = base(engine, vars, response);
    if let Some(slot) = request.and_then(super::request_body::recorded_bytes) {
        ctx.request_body = slot;
    }
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
    if let Some(form) = prep.logical.form() {
        ctx.request_headers.insert(
            super::multipart_body::CONTENT_TYPE.to_owned(),
            form.content_type().to_owned(),
        );
    }
    ctx.request_query = prep.url_result.query_params.clone();
    ctx.request_path = prep.url_result.path_params.clone();
    ctx.request_body = prep.logical.slot();
    ctx.blob_slots.request_body = prep.logical.blob().is_some();
    ctx
}

#[cfg(test)]
mod tests {
    use arazzo_expr::BodySlot;

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
    fn response_overlay_shares_the_decoded_body_instead_of_copying_it() {
        let engine = engine();
        let vars = VarStore::default();
        let absent = base(&engine, &vars, None);
        assert_eq!(absent.status_code, None);
        assert_eq!(absent.response_body, BodySlot::Absent);
        assert!(absent.response_headers.is_empty());
        let response = Response::decoded(
            201,
            BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("x-test".into(), "value".into()),
            ]),
            br#"{"a":1}"#.to_vec(),
            Vec::new(),
        );
        let first = base(&engine, &vars, Some(&response));
        let second = base(&engine, &vars, Some(&response));
        assert_eq!(first.status_code, Some(201));
        assert_eq!(first.response_headers, response.headers);
        let (BodySlot::Present(stored), BodySlot::Present(left), BodySlot::Present(right)) = (
            &response.body_value,
            &first.response_body,
            &second.response_body,
        ) else {
            panic!("decoded JSON body should be present")
        };
        assert!(Arc::ptr_eq(stored, left));
        assert!(Arc::ptr_eq(left, right));
        assert_eq!(**left, json!({"a": 1}));
        assert_shared(&absent, &first);
        let bytes = Response::decoded(200, BTreeMap::new(), vec![255], Vec::new());
        assert!(matches!(bytes.body_value, BodySlot::Unavailable { .. }));
        assert_eq!(
            base(&engine, &vars, Some(&bytes)).response_body,
            bytes.body_value
        );
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
