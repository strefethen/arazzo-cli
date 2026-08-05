#![forbid(unsafe_code)]

//! URL assembly for the Arazzo 1.1.0 `in: querystring` parameter location.
//!
//! A `querystring` parameter supplies the *entire* query component as a single
//! already-encoded value. Every rule that follows from that — verbatim
//! insertion, a tolerated leading `?`, replacement of a query the target
//! already carried, and refusal of a raw `#` — is pinned here, hermetically,
//! off the dry-run request plan.

use std::collections::BTreeMap;

use arazzo_runtime::{EngineBuilder, RuntimeErrorKind};
use arazzo_spec::{
    ArazzoSpec, Info, ParamLocation, Parameter, SourceDescription, SourceType, Step, StepTarget,
    Workflow,
};

const BASE: &str = "https://search.example.com/v1";

fn param(name: &str, location: ParamLocation, value: serde_yaml_ng::Value) -> Parameter {
    Parameter {
        name: name.to_string(),
        in_: Some(location),
        value: value.into(),
        ..Parameter::default()
    }
}

fn text(value: &str) -> serde_yaml_ng::Value {
    serde_yaml_ng::Value::String(value.to_string())
}

/// One step against `operation_path`, carrying `workflow_params` at the
/// workflow level and `step_params` at the step level.
fn spec_with(
    operation_path: &str,
    workflow_params: Vec<Parameter>,
    step_params: Vec<Parameter>,
) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.1.0".to_string(),
        info: Info {
            title: "querystring parameters".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "search".to_string(),
            url: BASE.to_string(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            parameters: workflow_params,
            steps: vec![Step {
                step_id: "probe".to_string(),
                target: Some(StepTarget::OperationPath(operation_path.to_string())),
                parameters: step_params,
                ..Step::default()
            }],
            ..Workflow::default()
        }],
        ..ArazzoSpec::default()
    }
}

struct Planned {
    url: String,
    warnings: Vec<String>,
}

fn plan(spec: ArazzoSpec) -> Result<Planned, (RuntimeErrorKind, String)> {
    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => panic!("building tokio runtime: {err}"),
    };
    let result = runtime.block_on(engine.execute_collect("wf", BTreeMap::new()));
    if let Err(err) = &result.outputs {
        return Err((err.kind, err.message.clone()));
    }
    let requests = result.dry_run_requests();
    assert_eq!(requests.len(), 1, "expected exactly one planned request");
    Ok(Planned {
        url: requests[0].url.clone(),
        warnings: requests[0].warnings.clone(),
    })
}

/// Plans a step whose only parameter is `in: querystring` with `value`.
fn plan_querystring(value: serde_yaml_ng::Value) -> Result<Planned, (RuntimeErrorKind, String)> {
    plan(spec_with(
        "{search}./index",
        Vec::new(),
        vec![param("filter", ParamLocation::Querystring, value)],
    ))
}

fn expect_plan(value: serde_yaml_ng::Value) -> Planned {
    match plan_querystring(value) {
        Ok(planned) => planned,
        Err((kind, message)) => {
            panic!("expected a planned request, got {}: {message}", kind.code())
        }
    }
}

/// The value becomes the query component as written: one `?`, no re-encoding of
/// the `=` and `&` that make it a query in the first place.
#[test]
fn querystring_value_becomes_the_query_component_verbatim() {
    let planned = expect_plan(text("q=red+shoes&limit=10"));

    assert_eq!(planned.url, format!("{BASE}/index?q=red+shoes&limit=10"));
    assert!(
        !planned.url.contains("%3D") && !planned.url.contains("%26"),
        "value was percent-encoded a second time: {}",
        planned.url
    );
    assert_eq!(
        planned.url.matches('?').count(),
        1,
        "expected exactly one '?': {}",
        planned.url
    );
    assert!(planned.warnings.is_empty(), "{:?}", planned.warnings);
}

/// The author may write the value with or without the delimiter; both mean the
/// same query, and neither doubles the `?`.
#[test]
fn leading_question_mark_is_tolerated_and_not_doubled() {
    let with_mark = expect_plan(text("?q=red+shoes&limit=10"));
    let without_mark = expect_plan(text("q=red+shoes&limit=10"));

    assert_eq!(with_mark.url, without_mark.url);
    assert_eq!(with_mark.url, format!("{BASE}/index?q=red+shoes&limit=10"));
}

/// Percent-escapes the author already applied survive untouched; encoding them
/// again would turn `%20` into `%2520`.
#[test]
fn already_encoded_escapes_are_not_encoded_again() {
    let planned = expect_plan(text("q=red%20shoes&tag=a%26b"));

    assert_eq!(planned.url, format!("{BASE}/index?q=red%20shoes&tag=a%26b"));
    assert!(!planned.url.contains("%25"), "{}", planned.url);
}

/// A raw `#` would end the query and start a fragment, silently truncating the
/// request. Only the author can say whether they meant `%23`, so this fails
/// rather than guessing.
#[test]
fn raw_fragment_delimiter_is_refused() {
    let Err((kind, message)) = plan_querystring(text("q=red#shoes")) else {
        panic!("expected a raw '#' in a querystring value to fail");
    };

    assert_eq!(kind, RuntimeErrorKind::InvalidParameterValue);
    assert_eq!(kind.code(), "RUNTIME_INVALID_PARAMETER_VALUE");
    assert!(
        message.contains("filter") && message.contains("%23"),
        "message should name the parameter and the encoding: {message}"
    );
}

/// The escaped form is a perfectly ordinary query value and is left alone.
#[test]
fn encoded_fragment_delimiter_is_accepted() {
    let planned = expect_plan(text("q=red%23shoes"));

    assert_eq!(planned.url, format!("{BASE}/index?q=red%23shoes"));
}

/// `querystring` *is* the query component, so it replaces one the target
/// already carried rather than being appended to it — and says so, because
/// dropping the author's query in silence is how a request goes wrong unnoticed.
#[test]
fn querystring_replaces_a_query_the_target_already_carried() {
    let planned = match plan(spec_with(
        "https://elsewhere.example.com/index?page=2&sort=asc",
        Vec::new(),
        vec![param(
            "filter",
            ParamLocation::Querystring,
            text("q=red&limit=10"),
        )],
    )) {
        Ok(planned) => planned,
        Err((kind, message)) => {
            panic!("expected a planned request, got {}: {message}", kind.code())
        }
    };

    assert_eq!(
        planned.url,
        "https://elsewhere.example.com/index?q=red&limit=10"
    );
    assert!(
        planned
            .warnings
            .iter()
            .any(|warning| warning.contains("page=2&sort=asc")),
        "expected the replaced query to be named: {:?}",
        planned.warnings
    );
}

/// An empty value means no query at all, and must not leave a bare `?` behind.
#[test]
fn empty_querystring_value_removes_the_query_entirely() {
    let planned = match plan(spec_with(
        "https://elsewhere.example.com/index?page=2",
        Vec::new(),
        vec![param("filter", ParamLocation::Querystring, text(""))],
    )) {
        Ok(planned) => planned,
        Err((kind, message)) => {
            panic!("expected a planned request, got {}: {message}", kind.code())
        }
    };

    assert_eq!(planned.url, "https://elsewhere.example.com/index");
}

/// A structure is not a query component. Stringifying it would send
/// `{"a":1}` and call the parameter resolved.
#[test]
fn non_string_value_warns_and_is_dropped() {
    let mapping = serde_yaml_ng::Value::Mapping({
        let mut mapping = serde_yaml_ng::Mapping::new();
        mapping.insert(text("q"), text("red"));
        mapping
    });
    let planned = expect_plan(mapping);

    assert_eq!(planned.url, format!("{BASE}/index"));
    assert!(
        planned.warnings.iter().any(|warning| {
            warning.contains("filter") && warning.contains("object") && warning.contains("dropped")
        }),
        "expected a typed warning naming the parameter: {:?}",
        planned.warnings
    );
    assert!(
        !planned.url.contains("q"),
        "the structure was stringified into the URL: {}",
        planned.url
    );
}

/// Null is skipped in silence, exactly as an unset `in: query` parameter is.
#[test]
fn null_querystring_value_is_skipped_silently() {
    let planned = expect_plan(serde_yaml_ng::Value::Null);

    assert_eq!(planned.url, format!("{BASE}/index"));
    assert!(planned.warnings.is_empty(), "{:?}", planned.warnings);
}

/// `merge_workflow_params` keys on `(name, in)`, so a workflow-level `query`
/// and a step-level `querystring` of the same name do not override one another
/// — both reach this step. `arazzo-validate` rejects that document; an
/// unvalidated spec reaching the engine still gets a defined URL, with the
/// dropped `query` parameter named.
#[test]
fn inherited_query_and_step_querystring_both_reach_the_step() {
    let planned = match plan(spec_with(
        "{search}./index",
        vec![param("q", ParamLocation::Query, text("inherited"))],
        vec![param(
            "q",
            ParamLocation::Querystring,
            text("q=step&limit=1"),
        )],
    )) {
        Ok(planned) => planned,
        Err((kind, message)) => {
            panic!("expected a planned request, got {}: {message}", kind.code())
        }
    };

    assert_eq!(planned.url, format!("{BASE}/index?q=step&limit=1"));
    assert!(
        planned
            .warnings
            .iter()
            .any(|warning| warning.contains("dropped") && warning.contains("q")),
        "expected the dropped query parameter to be named: {:?}",
        planned.warnings
    );
}

/// A workflow-level `querystring` is inherited like any other parameter, and a
/// step-level one of the same name overrides it rather than fighting with it.
#[test]
fn step_level_querystring_overrides_the_inherited_one() {
    let planned = match plan(spec_with(
        "{search}./index",
        vec![param(
            "filter",
            ParamLocation::Querystring,
            text("q=inherited"),
        )],
        vec![param("filter", ParamLocation::Querystring, text("q=step"))],
    )) {
        Ok(planned) => planned,
        Err((kind, message)) => {
            panic!("expected a planned request, got {}: {message}", kind.code())
        }
    };

    assert_eq!(planned.url, format!("{BASE}/index?q=step"));
}

/// The regression guard for every other location: adding `Querystring` to the
/// model must not have moved `path`, `query`, `header`, or `cookie`.
#[test]
fn other_parameter_locations_are_unchanged() {
    let planned = match plan(spec_with(
        "{search}./index/{id}",
        Vec::new(),
        vec![
            param("id", ParamLocation::Path, text("7")),
            param("q", ParamLocation::Query, text("red shoes")),
            param("X-Trace", ParamLocation::Header, text("abc")),
            param("session", ParamLocation::Cookie, text("xyz")),
        ],
    )) {
        Ok(planned) => planned,
        Err((kind, message)) => {
            panic!("expected a planned request, got {}: {message}", kind.code())
        }
    };

    // `query` still goes through form-urlencoding: the space becomes `+`.
    assert_eq!(planned.url, format!("{BASE}/index/7?q=red+shoes"));
}
