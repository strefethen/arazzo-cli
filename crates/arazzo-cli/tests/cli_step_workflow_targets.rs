#![allow(clippy::expect_used)]

use serde_json::Value;
use std::{fs, process::Command};

#[test]
fn external_workflow_warns_and_strict_promotes() {
    let path = std::env::temp_dir().join(format!(
        "arazzo-external-workflow-{}.yaml",
        std::process::id()
    ));
    fs::write(&path, "arazzo: 1.1.0\ninfo: {title: External, version: '1'}\nsourceDescriptions:\n  - {name: child, type: arazzo, url: absent-child.yaml}\nworkflows:\n  - workflowId: parent\n    steps:\n      - stepId: call\n        workflowId: $sourceDescriptions.child.flow\n").expect("fixture");
    for strict in [false, true] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"));
        cmd.arg("validate").arg(&path).arg("--json");
        if strict {
            cmd.arg("--strict");
        }
        let output = cmd.output().expect("validate");
        let body: Value = serde_json::from_slice(&output.stdout).expect("JSON stdout");
        assert_eq!(output.status.success(), !strict, "{body}");
        assert_eq!(body["valid"], !strict, "{body}");
        let issues = body[if strict { "errors" } else { "warnings" }]
            .as_array()
            .expect("issues");
        assert!(
            issues
                .iter()
                .any(|issue| issue["kind"] == "unsupportedWorkflowId"),
            "{body}"
        );
    }
    fs::remove_file(path).expect("cleanup fixture");
}
