use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use arazzo_runtime::{DebugController, Engine, StepBreakpoint};
use arazzo_spec::{ArazzoSpec, Info, SourceDescription, Step, Workflow};
use serde_json::json;
use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};

#[test]
fn evaluate_and_watch_expressions_at_pause() {
    let server = start_server();
    let mut engine = build_engine(server.base_url.clone());
    let controller = Arc::new(DebugController::new());
    if let Err(err) = controller.set_breakpoints(vec![StepBreakpoint::new("wf", "s2")]) {
        panic!("setting breakpoints: {err}");
    }
    engine.set_debug_controller(Arc::clone(&controller));

    let inputs = BTreeMap::from([(String::from("code"), json!(429))]);
    let handle = thread::spawn(move || engine.execute("wf", inputs));

    let waited = match controller.wait_for_stop_count(1, Duration::from_secs(1)) {
        Ok(value) => value,
        Err(err) => panic!("waiting for stop event: {err}"),
    };
    if !waited {
        let _ = controller.continue_execution();
        let _ = handle.join();
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

    let cond = match controller.evaluate_condition("$steps.s1.outputs.code == 429") {
        Ok(value) => value,
        Err(err) => panic!("evaluating condition expression: {err}"),
    };
    assert!(cond);

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
    let joined = match handle.join() {
        Ok(value) => value,
        Err(_) => panic!("execution thread panicked"),
    };
    if let Err(err) = joined {
        panic!("workflow execution failed: {err}");
    }
}

fn build_engine(url: String) -> Engine {
    let spec = ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "debug-evaluate".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url,
            type_: "openapi".to_string(),
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            summary: String::new(),
            description: String::new(),
            inputs: None,
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/echo".to_string(),
                    outputs: BTreeMap::from([("code".to_string(), "code".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/noop".to_string(),
                    ..Step::default()
                },
            ],
            outputs: BTreeMap::new(),
        }],
        components: None,
    };

    match Engine::new(spec) {
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
