#![forbid(unsafe_code)]

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{json, Value};

const MODES: [&str; 3] = ["off", "warn", "error"];
const SERIAL_WARNING: &str = "workflow \"serial\" output \"skippedStatus\": $steps.skipped.outputs.status: step \"skipped\" not found in context";
const CHILD_WARNING: &str = "workflow \"child\" output \"skippedStatus\": $steps.skipped.outputs.status: step \"skipped\" not found in context";
const STEP_WARNING: &str = "workflow \"serial\" step \"first\": output \"bad\": $steps.absent.outputs.value: step \"absent\" not found in context";

struct Fixture {
    dir: PathBuf,
    stopped: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
}

impl Fixture {
    fn new(step_warning: bool) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "arazzo-workflow-output-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("create fixture directory: {err}"));
        let server = tiny_http::Server::http("127.0.0.1:0")
            .unwrap_or_else(|err| panic!("start local server: {err}"));
        common::write_in(&dir, &format!("http://{}", server.server_addr()));
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopped);
        let server = thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                if let Some(request) = server
                    .recv_timeout(Duration::from_millis(20))
                    .unwrap_or_else(|err| panic!("receive fixture request: {err}"))
                {
                    request
                        .respond(tiny_http::Response::from_string("{}"))
                        .unwrap_or_else(|err| panic!("respond to fixture request: {err}"));
                }
            }
        });
        let mut first = json!({"stepId":"first", "operationId":"fixtureGetIndex",
            "onSuccess":[{"name":"stop", "type":"end"}]});
        if step_warning {
            first["outputs"] = json!({"bad":"$steps.absent.outputs.value"});
        }
        let serial = |id: &str| {
            json!({"workflowId":id,
            "steps":[first.clone(), {"stepId":"skipped", "operationId":"fixtureGetIndex"}],
            "outputs":{"skippedStatus":"$steps.skipped.outputs.status"}})
        };
        let spec = json!({"arazzo":"1.1.0", "info":{"title":"Output warnings", "version":"1"},
            "sourceDescriptions":[{"name":"api", "url":"./fixture.openapi.json", "type":"openapi"}],
            "workflows":[serial("serial"), serial("child"),
                {"workflowId":"parent", "steps":[
                    {"stepId":"invoke", "workflowId":"child"},
                    {"stepId":"fail", "operationId":"fixtureGetIndex",
                     "successCriteria":[{"condition":"$statusCode == 500"}]}]},
                {"workflowId":"outer", "steps":[{"stepId":"invoke", "workflowId":"child"}],
                 "outputs":{"missing":"$steps.invoke.outputs.nope"}}]});
        fs::write(dir.join("spec.yaml"), spec.to_string())
            .unwrap_or_else(|err| panic!("write fixture spec: {err}"));
        Self {
            dir,
            stopped,
            server: Some(server),
        }
    }

    fn run(&self, workflow: &str, mode: &str, json: bool, dry_run: bool) -> Output {
        let mut args = vec!["run", "spec.yaml", workflow, "--expr-diagnostics", mode];
        if json {
            args.push("--json");
        }
        if dry_run {
            args.push("--dry-run");
        }
        self.command(&args)
    }

    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args(args)
            .current_dir(&self.dir)
            .output()
            .unwrap_or_else(|err| panic!("execute CLI: {err}"))
    }

    fn stop_server(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(server) = self.server.take() {
            server
                .join()
                .unwrap_or_else(|err| panic!("join fixture server: {err:?}"));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop_server();
        fs::remove_dir_all(&self.dir)
            .unwrap_or_else(|err| panic!("remove fixture directory: {err}"));
    }
}

fn body(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "JSON: {err}; stdout={}; stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_line(output: &Output, warning: &str, count: usize) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = format!("warning: {warning}");
    assert_eq!(
        stderr.lines().filter(|actual| *actual == line).count(),
        count,
        "{stderr}"
    );
}

#[test]
fn run_json_live_dry_and_native_failure_preserve_workflow_warnings_in_every_mode() {
    let fixture = Fixture::new(false);
    for mode in MODES {
        for dry in [false, true] {
            let output = fixture.run("serial", mode, true, dry);
            let result = body(&output);
            assert_eq!(
                output.status.code(),
                Some(i32::from(mode == "error")),
                "{result}"
            );
            assert_eq!(result["warnings"], json!([SERIAL_WARNING]));
            assert_eq!(
                result["kind"],
                if mode == "error" {
                    "error"
                } else if dry {
                    "dryRun"
                } else {
                    "success"
                }
            );
            if mode == "error" {
                assert_eq!(result["code"], "RUNTIME_EXPRESSION_DIAGNOSTICS");
                assert_eq!(
                    result["error"],
                    "expression diagnostics reported 1 warning(s)"
                );
            } else if !dry {
                assert_eq!(result["outputs"], json!({"skippedStatus":null}));
            }
            assert_line(&output, SERIAL_WARNING, 0);
            assert!(!String::from_utf8_lossy(&output.stderr).contains("workflow output"));
        }
        let output = fixture.run("parent", mode, true, false);
        let result = body(&output);
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(result["kind"], "error");
        assert_eq!(result["code"], "RUNTIME_SUCCESS_CRITERIA_FAILED");
        assert_eq!(result["warnings"], json!([CHILD_WARNING]));
        assert_line(&output, CHILD_WARNING, 0);
    }
}

#[test]
fn run_json_orders_step_warnings_before_workflow_warnings_and_keeps_event_order() {
    let fixture = Fixture::new(true);
    for mode in MODES {
        let output = fixture.run("serial", mode, true, true);
        let result = body(&output);
        assert_eq!(
            result["warnings"],
            if mode == "off" {
                json!([SERIAL_WARNING])
            } else {
                json!([STEP_WARNING, SERIAL_WARNING])
            }
        );
        if mode == "error" {
            assert_eq!(
                result["error"],
                "expression diagnostics reported 2 warning(s)"
            );
        }
    }
    let output = fixture.run("serial", "warn", false, true);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_line(&output, STEP_WARNING, 1);
    assert_line(&output, SERIAL_WARNING, 1);
    assert!(
        stderr
            .find(STEP_WARNING)
            .unwrap_or_else(|| panic!("step warning missing: {stderr}"))
            < stderr
                .find(SERIAL_WARNING)
                .unwrap_or_else(|| panic!("workflow warning missing: {stderr}")),
        "{stderr}"
    );
    let fixture = Fixture::new(false);
    let output = fixture.run("outer", "off", true, false);
    assert!(output.status.success(), "{}", body(&output));
    assert_eq!(body(&output)["warnings"], json!([
        CHILD_WARNING,
        "workflow \"outer\" output \"missing\": $steps.invoke.outputs.nope: output \"nope\" not found in step \"invoke\""
    ]));
}

#[test]
fn human_run_prints_workflow_warning_once_on_success_escalation_and_native_failure() {
    let fixture = Fixture::new(false);
    for mode in MODES {
        for dry in [false, true] {
            let output = fixture.run("serial", mode, false, dry);
            assert_eq!(output.status.code(), Some(i32::from(mode == "error")));
            assert_line(&output, SERIAL_WARNING, 1);
        }
        let output = fixture.run("parent", mode, false, false);
        assert_eq!(output.status.code(), Some(1));
        assert_line(&output, CHILD_WARNING, 1);
    }
}

#[test]
fn test_reports_workflow_warning_and_escalates_only_otherwise_successful_cases() {
    let fixture = Fixture::new(false);
    for mode in ["off", "error"] {
        for workflow in ["serial", "parent"] {
            let filter = format!("^{workflow}$");
            let mut args = vec!["--json", "test", "spec.yaml", "--filter", &filter];
            if mode == "error" {
                args.extend(["--expr-diagnostics", mode]);
            }
            let output = fixture.command(&args);
            let result = body(&output);
            assert_eq!(result["kind"], "results");
            assert!(result.get("warnings").is_none());
            let case = &result["suites"][0]["tests"][0];
            assert!(case.get("warnings").is_none());
            if workflow == "parent" {
                assert_eq!(case["status"], "fail");
                assert_eq!(case["errorCode"], "RUNTIME_SUCCESS_CRITERIA_FAILED");
                assert_line(&output, CHILD_WARNING, 1);
            } else if mode == "error" {
                assert_eq!(case["status"], "error");
                assert_eq!(case["errorCode"], "RUNTIME_EXPRESSION_DIAGNOSTICS");
                assert_line(&output, SERIAL_WARNING, 1);
            } else {
                assert_eq!(case["status"], "pass");
                assert_line(&output, SERIAL_WARNING, 1);
            }
            assert_eq!(
                output.status.code(),
                Some(i32::from(workflow == "parent" || mode == "error"))
            );
        }
    }
}

#[test]
fn replay_recorded_trace_prints_warning_without_changing_json_contract() {
    let mut fixture = Fixture::new(false);
    let recorded = fixture.command(&[
        "--json",
        "run",
        "spec.yaml",
        "serial",
        "--trace",
        "trace.json",
    ]);
    assert!(recorded.status.success(), "{}", body(&recorded));
    fixture.stop_server();
    let replay = fixture.command(&["--json", "replay", "trace.json"]);
    assert!(replay.status.success(), "{}", body(&replay));
    assert_eq!(
        body(&replay),
        json!({"kind":"success", "outputs":{"skippedStatus":null}, "requestsChecked":1})
    );
    assert_line(&replay, SERIAL_WARNING, 1);
}
