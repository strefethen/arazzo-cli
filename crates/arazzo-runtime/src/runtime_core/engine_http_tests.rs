//! Unit coverage for the HTTP step path in [`super::engine_http`].
//!
//! `prepare_http_request` and `build_url_from_path` are `pub(crate)`, so no
//! integration target under `tests/` can call them — the rules they own are
//! provable only from inside the crate. This module sits beside
//! `helper_tests.rs` for that reason.
//!
//! Scope is the branches whole-workflow targets do not reach.
//! `operation_id_routing.rs` owns which server a resolved `operationId`
//! reaches, `querystring_parameter.rs` owns the `in: querystring` refusals,
//! and `operation_path_forms.rs` owns `operationPath` classification; none of
//! them exercise an object-valued `query` parameter, a parameter carrying no
//! `in:` location, a `querystring` meeting a target that already has a query,
//! or a bound operation's method regardless of request-body presence.
//!
//! The two rules here that need a response are proved through a replay engine
//! rather than a server, so the module stays hermetic.
//!
//! # Grouping
//!
//! Sections are the concern each rule belongs to, not the entry point that
//! reaches it, and they are named for the modules
//! `plans/current/arazzo-runtime-http-decomposition.md` proposes. A test moves
//! with the code it proves, so when that split lands each section is lifted
//! whole into its owner's `#[cfg(test)] mod tests` — only the fixtures below
//! are shared and need dividing. Add a test to the section that owns the rule,
//! even when the call that exercises it goes through another module's function.

use arazzo_spec::{Info, RequestBody, SourceDescription};

// `PreparedRequest` is the unit under test here but nothing outside the HTTP
// path consumes it, so it is imported from its owner rather than re-exported
// from `runtime_core` for the sake of a test.
use super::engine_http::PreparedRequest;
use super::*;

// ── Fixtures ────────────────────────────────────────────────────────

/// Reserved TLD (RFC 2606): nothing here sends a request, and a fixture that
/// starts to would fail to resolve rather than reach a real host.
const BASE: &str = "https://api.invalid";

fn spec_with(workflows: Vec<Workflow>) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "engine_http fixture".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: BASE.to_string(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows,
        components: None,
        ..ArazzoSpec::default()
    }
}

fn fixture_builder(spec: ArazzoSpec) -> EngineBuilder {
    let document = json!({"openapi": "3.2.0", "$self": BASE,
        "info": {"title": "inline HTTP", "version": "1"},
        "servers": [{"url": BASE}], "paths": {"/items": {"get": {}, "post": {}, "put": {}}, "/items/{id}": {"get": {}}, "/search": {"get": {}}}});
    EngineBuilder::new(spec).openapi_spec(document.to_string().into_bytes(), None)
}

fn fixture_engine() -> Engine {
    fixture_builder(spec_with(Vec::new()))
        .build()
        .unwrap_or_else(|err| panic!("building fixture engine: {err}"))
}

fn param(name: &str, in_: Option<ParamLocation>, value: &str) -> Parameter {
    Parameter {
        name: name.to_string(),
        in_,
        value: serde_yaml_ng::Value::String(value.to_string()).into(),
        ..Parameter::default()
    }
}

fn step_at(op_path: &str, parameters: Vec<Parameter>) -> Step {
    Step {
        step_id: "s1".to_string(),
        target: Some(StepTarget::OperationPath(op_path.to_string())),
        parameters,
        ..Step::default()
    }
}

fn build_url(engine: &Engine, op_path: &str, step: &Step, vars: &VarStore) -> UrlBuildResult {
    match engine.build_url_from_path(op_path, BASE, step, vars) {
        Ok(result) => result,
        Err(err) => panic!("build_url_from_path({op_path:?}): {err}"),
    }
}

fn prepare(engine: &Engine, step: &Step, vars: &VarStore) -> PreparedRequest {
    match engine.prepare_http_request(step, vars) {
        Ok(prepared) => prepared,
        Err(err) => panic!("prepare_http_request: {err}"),
    }
}

fn query_pairs(url: &str) -> Vec<(String, String)> {
    let parsed = match url_crate::Url::parse(url) {
        Ok(parsed) => parsed,
        Err(err) => panic!("parsing {url:?}: {err}"),
    };
    parsed.query_pairs().into_owned().collect()
}

// ── Parameter binding — future `http/parameters.rs` ─────────────────────

/// An object cannot be exploded into key-value pairs the way an array can, so
/// it is serialized whole. The pair carries one JSON document as its value,
/// not one pair per member.
#[test]
fn a_query_parameter_holding_an_object_is_sent_as_one_json_string() {
    let engine = fixture_engine();
    let mut vars = VarStore::default();
    vars.set_input("filter", json!({ "status": "open" }));

    let step = step_at(
        "{$sourceDescriptions.test.url}#/paths/~1items/get",
        vec![param(
            "filter",
            Some(ParamLocation::Query),
            "$inputs.filter",
        )],
    );
    let result = build_url(&engine, "/items", &step, &vars);

    assert_eq!(
        query_pairs(&result.url),
        vec![("filter".to_string(), r#"{"status":"open"}"#.to_string())],
        "the object is one value, not one pair per member: {}",
        result.url
    );
    assert_eq!(
        result.query_params.get("filter"),
        Some(&r#"{"status":"open"}"#.to_string()),
        "$request.query.filter must describe the query actually sent"
    );
}

/// An array is exploded, which is the contrasting rule — asserted here so the
/// object case above cannot be read as the general shape for structures.
#[test]
fn a_query_parameter_holding_an_array_is_exploded_into_one_pair_per_element() {
    let engine = fixture_engine();
    let mut vars = VarStore::default();
    vars.set_input("tag", json!(["a", "b"]));

    let step = step_at(
        "{$sourceDescriptions.test.url}#/paths/~1items/get",
        vec![param("tag", Some(ParamLocation::Query), "$inputs.tag")],
    );
    let result = build_url(&engine, "/items", &step, &vars);

    assert_eq!(
        query_pairs(&result.url),
        vec![
            ("tag".to_string(), "a".to_string()),
            ("tag".to_string(), "b".to_string()),
        ],
        "actual: {}",
        result.url
    );
    assert_eq!(
        result.query_params.get("tag"),
        Some(&"a,b".to_string()),
        "repeats collapse comma-joined for the expression context"
    );
}

/// `in` is optional on a Parameter Object — a reference parameter carries none
/// until it is resolved. A value with no location names no part of the request,
/// so it reaches neither the URL nor the headers rather than defaulting to one.
#[test]
fn a_parameter_with_no_in_location_reaches_no_part_of_the_request() {
    let engine = fixture_engine();
    let vars = VarStore::default();
    let step = step_at(
        "{$sourceDescriptions.test.url}#/paths/~1items/get",
        vec![param("stray", None, "value")],
    );

    let result = build_url(&engine, "/items", &step, &vars);
    assert_eq!(result.url, format!("{BASE}/items"));
    assert!(
        result.query_params.is_empty(),
        "actual: {:?}",
        result.query_params
    );
    assert!(
        result.path_params.is_empty(),
        "actual: {:?}",
        result.path_params
    );

    let prepared = prepare(&engine, &step, &vars);
    assert!(
        !prepared.headers.contains_key("stray"),
        "actual: {:?}",
        prepared.headers
    );
}

/// Every parameter is resolved once, by the site that owns its location.
/// URL assembly used to resolve all of them and `prepare_http_request` seeds
/// its warning list from that result, so a header or cookie value that warned
/// was reported once from each site.
#[test]
fn a_parameter_value_that_warns_is_reported_once_per_parameter() {
    let engine = fixture_engine();
    let vars = VarStore::default();

    for location in [
        ParamLocation::Header,
        ParamLocation::Cookie,
        ParamLocation::Query,
        ParamLocation::Path,
    ] {
        let step = step_at(
            "{$sourceDescriptions.test.url}#/paths/~1items~1%7Bid%7D/get",
            vec![param("probe", Some(location), "$inputs.absent")],
        );
        let warnings = prepare(&engine, &step, &vars).warnings;
        assert_eq!(
            warnings.len(),
            1,
            "{location:?}: one unresolvable value is one warning, got {warnings:?}"
        );
        assert!(
            warnings[0].starts_with("parameter \"probe\": "),
            "{location:?}: {warnings:?}"
        );
    }
}

/// A parameter carrying no `in:` binds nothing, but its value is still
/// resolved so an unresolvable one is not silently dropped — `arazzo-validate`
/// only advises on the missing `in`, so such a parameter reaches the runtime.
#[test]
fn a_parameter_with_no_in_location_still_reports_an_unresolvable_value_once() {
    let engine = fixture_engine();
    let vars = VarStore::default();
    let step = step_at(
        "{$sourceDescriptions.test.url}#/paths/~1items/get",
        vec![param("probe", None, "$inputs.absent")],
    );

    let warnings = prepare(&engine, &step, &vars).warnings;
    assert_eq!(warnings.len(), 1, "actual: {warnings:?}");
    assert!(
        warnings[0].starts_with("parameter \"probe\": "),
        "actual: {warnings:?}"
    );
}

// ── URL and query assembly — future `http/url.rs` ───────────────────────

/// Query parameters are appended to whatever query the resolved target already
/// carries, joined with `&`. Only `in: querystring` replaces it.
#[test]
fn query_parameters_append_to_a_target_that_already_carries_a_query() {
    let engine = fixture_engine();
    let vars = VarStore::default();
    let step = step_at(
        "{$sourceDescriptions.test.url}#/paths/~1search/get",
        vec![param("q", Some(ParamLocation::Query), "rust")],
    );

    let result = build_url(&engine, "/search?scope=all", &step, &vars);
    assert_eq!(result.url, format!("{BASE}/search?scope=all&q=rust"));
}

/// The component is written verbatim, so a repeated key stays repeated on the
/// wire. `$request.query.<name>` describes the same query through the runtime's
/// one-value-per-name map, which collapses the repeat comma-joined — the same
/// way duplicate `in: query` parameters collapse.
#[test]
fn a_querystring_repeating_one_key_stays_verbatim_and_collapses_for_expressions() {
    let engine = fixture_engine();
    let vars = VarStore::default();
    let step = step_at(
        "{$sourceDescriptions.test.url}#/paths/~1search/get",
        vec![param(
            "raw",
            Some(ParamLocation::Querystring),
            "tag=a&tag=b",
        )],
    );

    let result = build_url(&engine, "/search", &step, &vars);
    assert_eq!(
        result.url,
        format!("{BASE}/search?tag=a&tag=b"),
        "an already-encoded component is never re-encoded"
    );
    assert_eq!(
        result.query_params.get("tag"),
        Some(&"a,b".to_string()),
        "$request.query.tag must describe the query actually sent"
    );
}

/// A bare trailing `?` is an empty query component: nothing was declared twice,
/// so the `querystring` takes over instead of hitting the both-sides refusal
/// that a non-empty existing query triggers.
#[test]
fn a_querystring_takes_over_a_bare_trailing_question_mark() {
    let engine = fixture_engine();
    let vars = VarStore::default();
    let step = step_at(
        "{$sourceDescriptions.test.url}#/paths/~1items/get",
        vec![param("raw", Some(ParamLocation::Querystring), "page=2")],
    );

    let result = build_url(&engine, "/items?", &step, &vars);
    assert_eq!(result.url, format!("{BASE}/items?page=2"));
}

/// The negative case for the rule above: a target carrying a real query is two
/// declarations of the same component, and the step fails rather than picking
/// one.
#[test]
fn a_querystring_meeting_a_non_empty_target_query_is_refused() {
    let engine = fixture_engine();
    let vars = VarStore::default();
    let step = step_at(
        "{$sourceDescriptions.test.url}#/paths/~1items/get",
        vec![param("raw", Some(ParamLocation::Querystring), "page=2")],
    );

    let err = match engine.build_url_from_path("/items?page=1", BASE, &step, &vars) {
        Ok(result) => panic!("expected a refusal, built {}", result.url),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::InvalidParameterValue);
    assert!(
        err.message.contains("already carries a query"),
        "actual: {}",
        err.message
    );
}

// ── Request assembly — future `http/request.rs` ─────────────────────────

/// Request-body presence cannot change the selected operation method.
#[test]
fn the_bound_operation_method_is_independent_of_request_body() {
    let engine = fixture_engine();
    let vars = VarStore::default();

    let mut payload = serde_yaml_ng::Mapping::new();
    payload.insert(
        serde_yaml_ng::Value::String("name".to_string()),
        serde_yaml_ng::Value::String("widget".to_string()),
    );
    let mut with_body = step_at(
        "{$sourceDescriptions.test.url}#/paths/~1items/get",
        Vec::new(),
    );
    with_body.request_body = Some(RequestBody {
        content_type: "application/json".to_string(),
        payload: Some(serde_yaml_ng::Value::Mapping(payload).into()),
        ..RequestBody::default()
    });

    assert_eq!(prepare(&engine, &with_body, &vars).method, "GET");
    assert_eq!(
        prepare(
            &engine,
            &step_at(
                "{$sourceDescriptions.test.url}#/paths/~1items/get",
                Vec::new()
            ),
            &vars
        )
        .method,
        "GET"
    );
}

/// The operation itself selects PUT, independently of body presence.
#[test]
fn a_bound_put_operation_retains_its_method_with_a_body() {
    let engine = fixture_engine();
    let vars = VarStore::default();

    let mut step = step_at(
        "{$sourceDescriptions.test.url}#/paths/~1items/put",
        Vec::new(),
    );
    step.request_body = Some(RequestBody {
        content_type: "application/json".to_string(),
        payload: Some(serde_yaml_ng::Value::String("body".to_string()).into()),
        ..RequestBody::default()
    });

    assert_eq!(prepare(&engine, &step, &vars).method, "PUT");
}

/// An absent payload sends no body or Content-Type and preserves the method.
#[test]
fn a_request_body_with_no_payload_preserves_the_bound_method() {
    let engine = fixture_engine();
    let vars = VarStore::default();

    let mut step = step_at(
        "{$sourceDescriptions.test.url}#/paths/~1items/get",
        Vec::new(),
    );
    step.request_body = Some(RequestBody {
        content_type: "application/json".to_string(),
        payload: None,
        ..RequestBody::default()
    });

    let prepared = prepare(&engine, &step, &vars);
    assert_eq!(prepared.method, "GET");
    assert!(prepared.body.is_none(), "actual: {:?}", prepared.body);
    assert!(
        prepared.body_json.is_none(),
        "actual: {:?}",
        prepared.body_json
    );
    assert!(
        !prepared.headers.contains_key("Content-Type"),
        "no body means no Content-Type describing one: {:?}",
        prepared.headers
    );
}

/// Missing operation context cannot become an implicit request to a base URL.
#[test]
fn a_step_with_no_target_is_refused() {
    let engine = fixture_engine();
    let vars = VarStore::default();
    let step = Step {
        step_id: "s1".to_string(),
        ..Step::default()
    };

    let error = match engine.prepare_http_request(&step, &vars) {
        Err(error) => error,
        Ok(request) => panic!("missing target produced a request: {request:?}"),
    };
    assert_eq!(error.kind, RuntimeErrorKind::InternalError);
}

// ── Step orchestration — future `http/execute.rs` ───────────────────────

/// A dry run evaluates outputs against a synthetic response, and an output
/// whose expression cannot be resolved is named in the warning rather than
/// reported as an anonymous failure.
#[tokio::test]
async fn a_dry_run_names_the_output_whose_expression_warned() {
    let spec = spec_with(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath(
                "{$sourceDescriptions.test.url}#/paths/~1items/get".to_string(),
            )),
            outputs: BTreeMap::from([("token".to_string(), "$nope.thing".to_string().into())]),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    let engine = match fixture_builder(spec).dry_run(true).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building dry-run engine: {err}"),
    };

    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    let requests = result.dry_run_requests();
    let [request] = requests.as_slice() else {
        panic!(
            "expected exactly one dry-run request, got {}",
            requests.len()
        );
    };
    assert!(
        request
            .warnings
            .iter()
            .any(|warning| warning.starts_with(r#"output "token": "#)),
        "actual: {:?}",
        request.warnings
    );
}

/// A criterion whose expression warns is attributed by its position in
/// `successCriteria`, so an author with several criteria learns which one
/// warned. Two criteria are declared and only the second warns, which is what
/// makes the index assertion meaningful.
#[tokio::test]
async fn a_success_criterion_warning_carries_the_index_of_the_criterion() {
    let spec = spec_with(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath(
                "{$sourceDescriptions.test.url}#/paths/~1items/get".to_string(),
            )),
            success_criteria: vec![
                SuccessCriterion {
                    condition: "$statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                },
                // Warns on the left branch — the workflow declares no
                // `absent` input — then passes on the right, so the step
                // succeeds and the warning is the only thing under test.
                SuccessCriterion {
                    condition: "$inputs.absent == true || $statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                },
            ],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    // Replayed rather than served: the rule under test is how a warning is
    // attributed, and a trace supplies the response without a socket.
    let replayed = vec![TraceStepRecord {
        seq: 1,
        workflow_id: "wf".to_string(),
        step_id: "s1".to_string(),
        attempt: 1,
        kind: "http".to_string(),
        operation_path: "/items".to_string(),
        request: Some(TraceRequest {
            method: "GET".to_string(),
            url: format!("{BASE}/items"),
            ..TraceRequest::default()
        }),
        response: Some(TraceResponse {
            status_code: 200,
            content_type: ContentType::Json,
            body_bytes: 2,
            body: Some("{}".to_string()),
            ..TraceResponse::default()
        }),
        ..TraceStepRecord::default()
    }];
    let engine = match fixture_builder(spec)
        .trace(true)
        .replay_trace_steps(replayed)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building replay engine: {err}"),
    };

    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(err) = &result.outputs {
        panic!("expected the replayed step to succeed, got: {err}");
    }
    let steps = result.trace_steps();
    let [step] = steps.as_slice() else {
        panic!("expected exactly one trace step, got {}", steps.len());
    };
    let attributed: Vec<&String> = step
        .warnings
        .iter()
        .filter(|warning| warning.starts_with("successCriteria["))
        .collect();
    assert_eq!(
        attributed.len(),
        1,
        "only the second criterion warns: {:?}",
        step.warnings
    );
    assert!(
        attributed[0].starts_with("successCriteria[1]: "),
        "the warning must name the criterion that produced it: {:?}",
        attributed[0]
    );
}
