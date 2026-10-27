//! `--export-run` refuses to overwrite a local `asyncapi` source description,
//! the same protection it already gives `openapi` sources: the local-source
//! inventory now lists AsyncAPI files.
#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::unwrap_used)] // Fixture setup and assertions.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "arazzo-cli-export-asyncapi-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn write(&self, name: &str, data: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, data).expect("write fixture");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(args)
        .output()
        .expect("execute CLI")
}

const ASYNCAPI: &str = "asyncapi: 3.0.0\ninfo:\n  title: Orders\n  version: 1.0.0\nservers:\n  local:\n    host: 127.0.0.1:1\n    protocol: kafka\nchannels:\n  orderPlaced:\n    address: orders.placed\n    messages:\n      orderPlaced:\n        contentType: application/json\noperations:\n  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n";

#[test]
fn export_run_refuses_to_overwrite_a_local_asyncapi_source() {
    let temp = TempDir::new();
    let asyncapi = temp.write("orders.asyncapi.yaml", ASYNCAPI);
    let spec = temp.write(
        "spec.arazzo.yaml",
        "arazzo: 1.1.0\ninfo:\n  title: Receive fixture\n  version: 1.0.0\nsourceDescriptions:\n  - name: orders\n    url: ./orders.asyncapi.yaml\n    type: asyncapi\nworkflows:\n  - workflowId: await-order\n    steps:\n      - stepId: await\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n",
    );

    let output = cli(&[
        "--json",
        "run",
        spec.to_str().unwrap(),
        "await-order",
        "--export-run",
        asyncapi.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    let stdout: Value = serde_json::from_slice(&output.stdout).expect("JSON stdout");
    assert_eq!(stdout["kind"], "error");
    assert_eq!(stdout["code"], "RUN_EXPORT_PATH", "{stdout}");
    assert!(
        fs::read_to_string(&asyncapi)
            .unwrap()
            .starts_with("asyncapi:"),
        "the AsyncAPI source must be untouched"
    );
}
