//! Wire-level header-name semantics: a run-wide default header and a
//! step's `in: header` parameter naming the same field are the same
//! field (RFC 9110 field names are case-insensitive, which Arazzo
//! 1.1.0 cites for the `header` parameter location). The step value
//! replaces the default; the request never carries two field lines for
//! one singleton field such as `User-Agent`.
//!
//! Hermetic: the server binds 127.0.0.1 only.

mod common;

use arazzo_runtime::{ClientConfig, EngineBuilder};
use arazzo_spec::{ParamLocation, Parameter, Step, StepTarget, ValueSource, Workflow};
use common::{make_spec_with_base, success_200, to_yaml};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Records every raw field line, so duplicates stay visible (the shared
/// harness folds headers into a map, which would hide exactly the
/// defect under test).
struct FieldLineServer {
    base_url: String,
    lines: Arc<Mutex<Vec<(String, String)>>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FieldLineServer {
    fn start() -> Self {
        let server = match tiny_http::Server::http("127.0.0.1:0") {
            Ok(server) => server,
            Err(err) => panic!("binding test server: {err}"),
        };
        let base_url = format!("http://{}", server.server_addr());
        let lines: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_ref = Arc::clone(&lines);
        let handle = std::thread::spawn(move || {
            if let Ok(request) = server.recv() {
                let recorded: Vec<(String, String)> = request
                    .headers()
                    .iter()
                    .map(|h| {
                        (
                            h.field.as_str().as_str().to_string(),
                            h.value.as_str().to_string(),
                        )
                    })
                    .collect();
                if let Ok(mut slot) = lines_ref.lock() {
                    *slot = recorded;
                }
                let response = tiny_http::Response::from_string(r#"{"ok":true}"#);
                if request.respond(response).is_err() {
                    // Test helper: client may disconnect before reading.
                }
            }
        });
        Self {
            base_url,
            lines,
            handle: Some(handle),
        }
    }

    fn finish(mut self) -> Vec<(String, String)> {
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                panic!("test server thread panicked");
            }
        }
        let slot = match self.lines.lock() {
            Ok(slot) => slot,
            Err(err) => panic!("reading recorded headers: {err}"),
        };
        slot.clone()
    }
}

fn values_for<'a>(lines: &'a [(String, String)], name: &str) -> Vec<&'a str> {
    lines
        .iter()
        .filter(|(field, _)| field.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .collect()
}

fn header_param(name: &str, value: &str) -> Parameter {
    Parameter {
        name: name.to_string(),
        in_: Some(ParamLocation::Header),
        value: ValueSource::Literal(to_yaml(json!(value))),
        ..Parameter::default()
    }
}

async fn run_with(defaults: BTreeMap<String, String>, parameters: Vec<Parameter>, base_url: &str) {
    let workflow = Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/x".to_string())),
            parameters,
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    };
    let spec = make_spec_with_base(base_url, vec![workflow]);
    let config = ClientConfig {
        default_headers: defaults,
        ..ClientConfig::default()
    };
    let engine = match EngineBuilder::new(spec).client_config(config).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let _ = engine.execute_collect("wf", BTreeMap::new()).await;
}

/// The built-in `User-Agent` default must not survive alongside a
/// step-declared one: two `User-Agent` field lines is the shape this
/// test rejects. `User-Agent` is a singleton field, so a recipient may
/// not combine the lines, and the pair would be sent as
/// `arazzo-cli/0.1, mine/9.9`.
#[tokio::test]
async fn step_header_param_replaces_default_user_agent() {
    let server = FieldLineServer::start();
    let base_url = server.base_url.clone();
    run_with(
        ClientConfig::default().default_headers,
        vec![header_param("User-Agent", "mine/9.9")],
        &base_url,
    )
    .await;

    let lines = server.finish();
    assert_eq!(
        values_for(&lines, "User-Agent"),
        vec!["mine/9.9"],
        "step parameter replaces the default agent, no second field line"
    );
}

/// Same field, different spelling: the match is case-insensitive, so a
/// lowercase step parameter still replaces the `User-Agent` default
/// rather than adding a `user-agent` line beside it.
#[tokio::test]
async fn step_header_param_replaces_default_case_insensitively() {
    let server = FieldLineServer::start();
    let base_url = server.base_url.clone();
    run_with(
        ClientConfig::default().default_headers,
        vec![header_param("user-agent", "mine/9.9")],
        &base_url,
    )
    .await;

    let lines = server.finish();
    assert_eq!(
        values_for(&lines, "User-Agent"),
        vec!["mine/9.9"],
        "case-variant step parameter replaces the default agent"
    );
}

/// The same rule for a caller-supplied default (the CLI's `-H`): a step
/// parameter naming that field wins, and only one line is sent.
#[tokio::test]
async fn step_header_param_replaces_caller_default() {
    let server = FieldLineServer::start();
    let base_url = server.base_url.clone();
    let mut defaults = ClientConfig::default().default_headers;
    defaults.insert("X-Trace".to_string(), "from-flag".to_string());
    run_with(
        defaults,
        vec![header_param("x-trace", "from-step")],
        &base_url,
    )
    .await;

    let lines = server.finish();
    assert_eq!(
        values_for(&lines, "X-Trace"),
        vec!["from-step"],
        "step parameter replaces a caller-supplied default header"
    );
}

/// A default the step does not name still reaches the wire — replacement
/// is per field, not a wholesale drop of the defaults.
#[tokio::test]
async fn unnamed_defaults_survive_a_step_header_param() {
    let server = FieldLineServer::start();
    let base_url = server.base_url.clone();
    let mut defaults = ClientConfig::default().default_headers;
    defaults.insert("X-Trace".to_string(), "from-flag".to_string());
    run_with(
        defaults,
        vec![header_param("User-Agent", "mine/9.9")],
        &base_url,
    )
    .await;

    let lines = server.finish();
    assert_eq!(values_for(&lines, "X-Trace"), vec!["from-flag"]);
    assert_eq!(values_for(&lines, "User-Agent"), vec!["mine/9.9"]);
}
