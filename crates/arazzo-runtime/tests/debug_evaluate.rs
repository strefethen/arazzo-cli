use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use arazzo_runtime::{DebugController, EngineBuilder, StepBreakpoint};
use arazzo_spec::{
    ArazzoSpec, ExpressionType, Info, OutputValue, SelectorObject, SelectorType, SourceDescription,
    SourceType, Step, StepTarget, Workflow,
};
use serde_json::json;
use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn evaluate_and_watch_expressions_at_pause() {
    let server = start_server();
    let controller = Arc::new(DebugController::new());
    let engine = build_engine(server.base_url.clone(), Arc::clone(&controller));
    if let Err(err) = controller.set_breakpoints(vec![StepBreakpoint::new("wf", "s2")]) {
        panic!("setting breakpoints: {err}");
    }

    let inputs = BTreeMap::from([
        (String::from("code"), json!(429)),
        (
            String::from("doc"),
            json!({"value": "one", "items": [1, 2]}),
        ),
        (String::from("other"), json!({"value": "two"})),
        (
            String::from("xml"),
            json!("<root><value>one</value></root>"),
        ),
    ]);
    let handle = engine.execute("wf", inputs);

    let waited = match controller.wait_for_stop_count(1, Duration::from_secs(1)) {
        Ok(value) => value,
        Err(err) => panic!("waiting for stop event: {err}"),
    };
    if !waited {
        let _ = controller.continue_execution();
        let _ = handle.collect().await;
        panic!("timed out waiting for pause at s2");
    }

    let input_value = match controller.evaluate_expression("$inputs.code") {
        Ok(value) => value,
        Err(err) => panic!("evaluating input expression: {err}"),
    };
    assert_eq!(input_value, json!(429));

    let step_value = match controller.evaluate_expression("$steps.s1.outputs.code") {
        Ok(value) => value,
        Err(err) => panic!("evaluating step expression: {err}"),
    };
    assert_eq!(step_value, json!(429));

    let selector = |context: &str, query: &str, type_: &str, version: &str| {
        OutputValue::Selector(SelectorObject {
            context: context.to_owned(),
            selector: query.to_owned(),
            type_: SelectorType::ExpressionType(ExpressionType {
                type_: type_.to_owned(),
                version: version.to_owned(),
                extensions: BTreeMap::new(),
            }),
            extensions: BTreeMap::new(),
        })
    };
    for (output, expected) in [
        (
            OutputValue::RuntimeExpression("$inputs.code".to_owned()),
            json!(429),
        ),
        (
            selector("$inputs.doc", "/value", "jsonpointer", ""),
            json!("one"),
        ),
        (
            selector("$inputs.other", "/value", "jsonpointer", ""),
            json!("two"),
        ),
        (
            selector("$inputs.doc", "$.items[*]", "jsonpath", "rfc9535"),
            json!([1, 2]),
        ),
        (
            selector("$inputs.doc", "$.items[*]", "jsonpath", "goessner"),
            json!(null),
        ),
        (
            selector("$inputs.xml", "//value", "xpath", "xpath-10"),
            json!("one"),
        ),
        (
            selector("$inputs.xml", "//value", "xpath", "xpath-30"),
            json!(null),
        ),
        (
            selector("$inputs.absent", "/value", "jsonpointer", ""),
            json!(null),
        ),
    ] {
        assert_eq!(
            controller.evaluate_output_value(&output),
            Ok(expected),
            "{output:?}"
        );
    }
    assert_eq!(
        controller.evaluate_watch_expression("missing_alias"),
        Ok(json!(null))
    );

    let cond = match controller.evaluate_condition("$steps.s1.outputs.code == 429") {
        Ok(value) => value,
        Err(err) => panic!("evaluating condition expression: {err}"),
    };
    assert!(cond);
    assert_eq!(
        controller.try_evaluate_condition_expression("$steps.s1.outputs.code == 429"),
        Ok(Some(true))
    );
    assert_eq!(
        controller
            .try_evaluate_condition_expression("$inputs.doc#/value != null")
            .unwrap_or_else(|err| panic!("evaluating json pointer condition: {err}")),
        Some(true)
    );
    assert_eq!(
        controller.try_evaluate_condition_expression("$inputs.doc"),
        Ok(None)
    );
    assert_eq!(
        controller.try_evaluate_condition_expression("//value"),
        Ok(None)
    );

    let false_decision = controller.evaluate_condition("false");
    // Dots belong to an input's exact name; grouping makes this property access.
    let invalid = "'é' == 'É' && ($inputs.code).x == 1";
    let invalid_decision = controller.evaluate_condition(invalid);
    let non_boolean_decision = controller.evaluate_condition("1");

    let watches = match controller.evaluate_watches(&[
        "$inputs.code".to_string(),
        "$steps.s1.outputs.code".to_string(),
    ]) {
        Ok(values) => values,
        Err(err) => panic!("evaluating watches: {err}"),
    };
    assert_eq!(watches.len(), 2);
    assert_eq!(watches[0].expression, "$inputs.code");
    assert_eq!(watches[0].value, json!(429));
    assert_eq!(watches[1].expression, "$steps.s1.outputs.code");
    assert_eq!(watches[1].value, json!(429));

    if let Err(err) = controller.continue_execution() {
        panic!("continuing execution: {err}");
    }
    let result = handle.collect().await;
    if let Err(err) = result.outputs {
        panic!("workflow execution failed: {err}");
    }
    assert_eq!(false_decision, Ok(false));
    assert_eq!(
        invalid_decision,
        Err(format!(
            "invalid simple condition at byte {}: property access requires an object",
            invalid
                .find(".x")
                .unwrap_or_else(|| panic!("access offset"))
        ))
    );
    assert_eq!(
        non_boolean_decision,
        Err("invalid simple condition at byte 0: boolean or null required".to_owned())
    );
}

fn build_engine(url: String, controller: Arc<DebugController>) -> arazzo_runtime::Engine {
    let spec = ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "debug-evaluate".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url,
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("/echo".to_string())),
                    outputs: BTreeMap::from([(
                        "code".to_string(),
                        "$response.body#/code".to_string().into(),
                    )]),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/noop".to_string())),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
        components: None,
        ..ArazzoSpec::default()
    };

    match EngineBuilder::new(spec)
        .debug_controller(controller)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("creating engine: {err}"),
    }
}

#[derive(Debug)]
struct TestServer {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn start_server() -> TestServer {
    let server = match Server::http("127.0.0.1:0") {
        Ok(server) => server,
        Err(err) => panic!("binding debug evaluate server: {err}"),
    };
    let base_url = format!("http://{}", server.server_addr());
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);

    let handle = thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            match server.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(request)) => {
                    let (status, body) = if request.url().contains("/echo") {
                        (200, "{\"code\":429}")
                    } else {
                        (200, "{}")
                    };
                    let mut response =
                        TinyResponse::from_string(body).with_status_code(StatusCode(status));
                    if let Ok(header) =
                        Header::from_bytes(b"Content-Type".as_slice(), b"application/json")
                    {
                        response = response.with_header(header);
                    }
                    let _ = request.respond(response);
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
    });

    TestServer {
        base_url,
        stop,
        handle: Some(handle),
    }
}
