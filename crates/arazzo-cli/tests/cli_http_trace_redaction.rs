#![forbid(unsafe_code)]

mod common;

use serde_json::Value;
use std::{fs, process::Command, thread, time::Duration};

#[test]
fn persisted_http_preview_masks_secrets_without_masking_live_outputs() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap_or_else(|_| panic!("bind loopback"));
    let dir = std::env::temp_dir().join(format!("arazzo-http-preview-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap_or_else(|_| panic!("fixture directory"));
    common::write_in(&dir, &format!("http://{}", server.server_addr()));
    let body = format!(
        r#"{{"a_token":"FAKE-HTTP-CANARY","b_nested":{{"password":"FAKE-NESTED-CANARY"}},"c_name":"visible","zz_padding":"{}"}}"#,
        "é".repeat(3000)
    );
    let body_bytes = body.len();
    let handle = thread::spawn(move || {
        for _ in 0..3 {
            let request = server
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|_| panic!("receive"))
                .unwrap_or_else(|| panic!("CLI request"));
            request
                .respond(
                    tiny_http::Response::from_string(&body).with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json")
                            .unwrap_or_else(|_| panic!("header")),
                    ),
                )
                .unwrap_or_else(|_| panic!("response"));
        }
    });
    let spec = dir.join("workflow.yaml");
    fs::write(&spec, "arazzo: 1.0.1\ninfo:\n  title: HTTP preview\n  version: 1.0.0\nsourceDescriptions:\n  - name: api\n    url: fixture.openapi.json\n    type: openapi\nworkflows:\n  - workflowId: wf\n    steps:\n      - stepId: one\n        operationId: fixtureGetOne\n        outputs:\n          token: $response.body#/a_token\n    outputs:\n      token: $steps.one.outputs.token\n").unwrap_or_else(|_| panic!("write workflow"));
    let mut artifacts = Vec::new();
    for limit in [0, 2048, 4096] {
        let trace = dir.join(format!("trace-{limit}.json"));
        let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args(["--json", "run"])
            .arg(&spec)
            .args(["wf", "--trace"])
            .arg(&trace)
            .args(["--trace-max-body-bytes", &limit.to_string()])
            .current_dir(&dir)
            .output()
            .unwrap_or_else(|_| panic!("run CLI"));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let live: Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("live JSON"));
        assert_eq!(live["outputs"]["token"], "FAKE-HTTP-CANARY");
        artifacts.push((
            limit,
            fs::read_to_string(trace).unwrap_or_else(|_| panic!("persisted trace")),
        ));
    }
    handle.join().unwrap_or_else(|_| panic!("fixture thread"));
    for (limit, raw) in artifacts {
        assert!(!raw.contains("FAKE-HTTP-CANARY"));
        assert!(!raw.contains("FAKE-NESTED-CANARY"));
        let trace: Value = serde_json::from_str(&raw).unwrap_or_else(|_| panic!("trace JSON"));
        let response = &trace["steps"][0]["response"];
        assert_eq!(response["bodyBytes"], body_bytes);
        assert!(response.get("bodyLossy").is_none());
        assert_eq!(response["bodyPreview"], response["body"]);
        let preview = response["bodyPreview"]
            .as_str()
            .unwrap_or_else(|| panic!("preview"));
        assert!(preview.len() <= limit + 3);
        if limit > 0 {
            assert!(preview.contains("[REDACTED]"));
            assert!(preview.contains("visible"));
        }
    }
}
