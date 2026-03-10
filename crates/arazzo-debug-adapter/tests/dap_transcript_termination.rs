#![forbid(unsafe_code)]

mod dap_test_support;

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arazzo_debug_adapter::run_dap_stdio;
use serde_json::json;
use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};

/// Stepping past the last checkpoint of the last step should emit a
/// `terminated` event without requiring a `disconnect` request.
/// This reproduces the VS Code hang where the debug toolbar stays
/// visible with disabled buttons after stepping off the final output.
#[test]
fn stepping_off_last_checkpoint_emits_terminated() {
    let server = start_server();
    let spec_path = write_temp_spec(&server.base_url);

    // Checkpoints for single-step spec (1 criterion, 2 outputs):
    //   0: Step          (stop on entry → reason "pause")
    //   1: SuccessCrit   (step over → reason "step")
    //   2: Output title_1 (step over → reason "step")
    //   3: Output link_1  (step over → reason "step")
    //   4: next           → engine finishes → terminated event
    let input = dap_test_support::encode_dap_stream(&[
        json!({ "seq": 1, "type": "request", "command": "initialize", "arguments": {} }),
        json!({
            "seq": 2,
            "type": "request",
            "command": "launch",
            "arguments": {
                "spec": spec_path.to_string_lossy(),
                "workflowId": "single-step",
                "stopOnEntry": true
            }
        }),
        json!({ "seq": 3, "type": "request", "command": "configurationDone", "arguments": {} }),
        // Step over from entry → criterion
        json!({ "seq": 4, "type": "request", "command": "next", "arguments": {} }),
        // Step over from criterion → output title_1
        json!({ "seq": 5, "type": "request", "command": "next", "arguments": {} }),
        // Step over from title_1 → output link_1
        json!({ "seq": 6, "type": "request", "command": "next", "arguments": {} }),
        // Step over from link_1 → engine finishes (no more checkpoints)
        json!({ "seq": 7, "type": "request", "command": "next", "arguments": {} }),
        // NO disconnect — the terminated event must arrive on its own.
    ]);

    let reader = Cursor::new(input);
    let mut output = Vec::<u8>::new();

    let run = run_dap_stdio(reader, &mut output);
    assert!(run.is_ok(), "DAP loop should exit cleanly: {run:?}");

    let messages = dap_test_support::decode_dap_stream(&output);

    // Verify we got exactly 4 stopped events (entry + 3 step overs).
    let stopped: Vec<_> = messages
        .iter()
        .filter(|m| m.get("event").and_then(|v| v.as_str()) == Some("stopped"))
        .collect();
    assert_eq!(
        stopped.len(),
        4,
        "expected 4 stopped events (entry + criterion + 2 outputs), got {}: {stopped:#?}",
        stopped.len()
    );

    // The critical assertion: a `terminated` event MUST be present.
    let terminated = messages
        .iter()
        .any(|m| m.get("event").and_then(|v| v.as_str()) == Some("terminated"));
    assert!(
        terminated,
        "terminated event must be emitted after stepping off last checkpoint.\n\
         Messages received ({}):\n{:#?}",
        messages.len(),
        messages
    );

    let _ = fs::remove_file(spec_path);
}

fn write_temp_spec(base_url: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let path = std::env::temp_dir().join(format!("arazzo-debug-termination-{nanos}.yaml"));
    let spec = format!(
        r#"
arazzo: "1.0.0"
info:
  title: Termination Test
  version: "1.0.0"
sourceDescriptions:
  - name: test
    url: {base_url}
    type: openapi
workflows:
  - workflowId: single-step
    steps:
      - stepId: fetch-data
        operationPath: /data
        successCriteria:
          - condition: $statusCode == 200
        outputs:
          title_1: //item[1]/title
          link_1: //item[1]/link
"#
    );
    fs::write(&path, spec).unwrap_or_else(|err| panic!("writing temp spec: {err}"));
    path
}

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
        Err(err) => panic!("binding termination test server: {err}"),
    };
    let base_url = format!("http://{}", server.server_addr());
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            match server.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(request)) => {
                    let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss>
  <channel>
    <item><title>one</title><link>https://example.com/one</link></item>
  </channel>
</rss>"#;
                    let mut response =
                        TinyResponse::from_string(body).with_status_code(StatusCode(200));
                    if let Ok(header) =
                        Header::from_bytes(b"Content-Type".as_slice(), b"application/rss+xml")
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
